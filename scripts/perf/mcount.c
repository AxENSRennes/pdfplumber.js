// LD_PRELOAD counter of malloc/calloc/realloc/free calls for the program
// named by MCOUNT_EXE; prints the counts to stderr at exit. With MCOUNT_OUT,
// also the call stack (frame pointers) of every 16th malloc or realloc, in
// sprof.c's format (a fixed 32 MiB buffer), for scripts/perf/sprof_report.py:
// where the allocations come from.
//
//   gcc -O2 -fno-omit-frame-pointer -shared -fPIC -o mcount.so scripts/perf/mcount.c -ldl
//   MCOUNT_EXE=$BIN [MCOUNT_OUT=allocs.bin] LD_PRELOAD=./mcount.so \
//     timeout 15m prlimit --as=8589934592 -- $BIN ...
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdint.h>
#include <sys/mman.h>

#define MAXD 40
#define CAP (1u << 22)
static uint64_t *sbuf;
static size_t sused;
static unsigned long every = 16;

static void sample(void) {
  if (!sbuf || sused + MAXD + 2 >= CAP) return;
  uint64_t *fp = (uint64_t *)__builtin_frame_address(0);
  size_t start = sused;
  uint32_t d = 0;
  uint64_t *out = sbuf + start + 1;
  while (d < MAXD && fp && ((uintptr_t)fp & 7) == 0) {
    uint64_t ret = fp[1];
    if (ret < 4096) break;
    out[d++] = ret;
    uint64_t *next = (uint64_t *)fp[0];
    if (next <= fp || (char *)next - (char *)fp > (64 << 20)) break;
    fp = next;
  }
  sbuf[start] = d;
  sused = start + 1 + d;
}

static void dump(void) {
  const char *path = getenv("MCOUNT_OUT");
  if (!path || !sbuf) return;
  int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0) return;
  size_t n = sused * 8, off = 0;
  while (off < n) { ssize_t w = write(fd, (char *)sbuf + off, n - off); if (w <= 0) break; off += w; }
  close(fd);
  char mp[4096];
  snprintf(mp, sizeof mp, "%s.maps", path);
  int in = open("/proc/self/maps", O_RDONLY), o = open(mp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  char tmp[65536]; ssize_t r;
  while (in >= 0 && o >= 0 && (r = read(in, tmp, sizeof tmp)) > 0) { if (write(o, tmp, r) != r) break; }
  if (in >= 0) close(in);
  if (o >= 0) close(o);
}

static void *(*real_malloc)(size_t);
static void *(*real_calloc)(size_t, size_t);
static void *(*real_realloc)(void *, size_t);
static void (*real_free)(void *);
static unsigned long n_malloc, n_calloc, n_realloc, n_free;
static int active;
static char tmpbuf[65536];
static size_t tmppos;

static void report(void) {
  if (active) dump();
  if (active)
    fprintf(stderr, "mcount: malloc %lu calloc %lu realloc %lu free %lu\n", n_malloc, n_calloc, n_realloc, n_free);
}

__attribute__((constructor)) static void init(void) {
  real_malloc = dlsym(RTLD_NEXT, "malloc");
  real_calloc = dlsym(RTLD_NEXT, "calloc");
  real_realloc = dlsym(RTLD_NEXT, "realloc");
  real_free = dlsym(RTLD_NEXT, "free");
  const char *want = getenv("MCOUNT_EXE");
  char exe[4096];
  ssize_t k = readlink("/proc/self/exe", exe, sizeof exe - 1);
  if (want && k > 0) {
    exe[k] = 0;
    active = strcmp(exe, want) == 0;
  }
  if (active && getenv("MCOUNT_OUT")) {
    void *m = mmap(0, CAP * 8, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m != MAP_FAILED) sbuf = m;
  }
  atexit(report);
}

void *malloc(size_t n) {
  if (!real_malloc) {  // dlsym's own allocations
    void *p = tmpbuf + tmppos;
    tmppos += (n + 15) & ~15UL;
    return p;
  }
  n_malloc++;
  if (sbuf && n_malloc % every == 0) sample();
  return real_malloc(n);
}
void *calloc(size_t a, size_t b) {
  if (!real_calloc) {
    void *p = tmpbuf + tmppos;
    tmppos += (a * b + 15) & ~15UL;
    memset(p, 0, a * b);
    return p;
  }
  n_calloc++;
  return real_calloc(a, b);
}
void *realloc(void *p, size_t n) {
  n_realloc++;
  if (sbuf && n_realloc % every == 0) sample();
  return real_realloc(p, n);
}
void free(void *p) {
  if ((char *)p >= tmpbuf && (char *)p < tmpbuf + sizeof tmpbuf) return;
  n_free++;
  if (real_free) real_free(p);
}
