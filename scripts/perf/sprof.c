// A sampling profiler for the native binaries where `perf` is not allowed
// (perf_event_paranoid; docs/architecture.md, "Performance"): loaded with
// LD_PRELOAD, active only in the program SPROF_EXE names (a timer survives
// exec), SIGPROF every SPROF_US microseconds of CPU time (500 by default),
// walks the frame pointers (build with RUSTFLAGS="-C force-frame-pointers=yes",
// and CARGO_PROFILE_RELEASE_DEBUG=line-tables-only for sprof_lines.py) and
// records up to 48 return addresses per sample in a fixed 32 MiB buffer
// (bounded memory: samples past it are dropped). At exit, writes SPROF_OUT
// (per sample: u64 depth, then the addresses) and SPROF_OUT.maps (a copy of
// /proc/self/maps); scripts/perf/sprof_report.py reads them.
//
//   gcc -O2 -shared -fPIC -o sprof.so scripts/perf/sprof.c
//   SPROF_EXE=$BIN SPROF_OUT=out.bin LD_PRELOAD=./sprof.so \
//     timeout 15m prlimit --as=8589934592 -- $BIN ...
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <ucontext.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>

#define MAXD 48
#define CAP (1u << 22) /* u64 words: 32 MiB */
static uint64_t *buf;
static volatile size_t used;
static volatile int on;
static uintptr_t stack_lo, stack_hi;

static void handler(int sig, siginfo_t *si, void *ctx) {
  (void)sig; (void)si;
  if (!on) return;
  ucontext_t *uc = (ucontext_t *)ctx;
  uint64_t ip = uc->uc_mcontext.gregs[REG_RIP];
  uint64_t fp = uc->uc_mcontext.gregs[REG_RBP];
  uint64_t sp = uc->uc_mcontext.gregs[REG_RSP];
  if (used + MAXD + 2 >= CAP) return;
  size_t start = used;
  uint64_t *out = buf + start + 1;
  uint32_t d = 0;
  out[d++] = ip;
  while (d < MAXD && fp > sp && fp < sp + (64u << 20) && (fp & 7) == 0) {
    uint64_t *f = (uint64_t *)fp;
    uint64_t ret = f[1];
    if (ret < 4096) break;
    out[d++] = ret;
    if (f[0] <= fp) break;
    fp = f[0];
  }
  buf[start] = d;
  used = start + 1 + d;
}

static void dump(void) {
  on = 0;
  const char *path = getenv("SPROF_OUT");
  if (!path) return;
  int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0) return;
  size_t n = used * 8, off = 0;
  while (off < n) { ssize_t w = write(fd, (char *)buf + off, n - off); if (w <= 0) break; off += w; }
  close(fd);
  char mp[4096];
  snprintf(mp, sizeof mp, "%s.maps", path);
  int in = open("/proc/self/maps", O_RDONLY), o = open(mp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  char tmp[65536]; ssize_t r;
  while (in >= 0 && o >= 0 && (r = read(in, tmp, sizeof tmp)) > 0) { if (write(o, tmp, r) != r) break; }
  if (in >= 0) close(in);
  if (o >= 0) close(o);
}

__attribute__((constructor)) static void init(void) {
  if (!getenv("SPROF_OUT") || !getenv("SPROF_EXE")) return;
  char exe[4096];
  ssize_t k = readlink("/proc/self/exe", exe, sizeof exe - 1);
  if (k <= 0) return;
  exe[k] = 0;
  if (strcmp(exe, getenv("SPROF_EXE")) != 0) return;
  buf = mmap(0, CAP * 8, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (buf == MAP_FAILED) return;
  struct sigaction sa;
  memset(&sa, 0, sizeof sa);
  sa.sa_sigaction = handler;
  sa.sa_flags = SA_SIGINFO | SA_RESTART;
  sigaction(SIGPROF, &sa, 0);
  int us = getenv("SPROF_US") ? atoi(getenv("SPROF_US")) : 500;
  struct itimerval it = {{0, us}, {0, us}};
  setitimer(ITIMER_PROF, &it, 0);
  on = 1;
  atexit(dump);
}
