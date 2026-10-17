//! The table of long-lived objects JavaScript refers to by handle
//! (documents, pages, derived pages, text maps).
//!
//! A handle is a `u32`: the slot index in its low 20 bits and the slot's
//! generation in the high 12 bits, so a handle used after its object was
//! freed (and its slot reused) is refused instead of reaching another
//! object.

const INDEX_BITS: u32 = 20;
const INDEX_MASK: u32 = (1 << INDEX_BITS) - 1;
const GEN_MASK: u32 = (1 << (32 - INDEX_BITS)) - 1;

/// The most objects alive at once.
pub const MAX_HANDLES: usize = INDEX_MASK as usize;

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// Objects by handle. Handle 0 is never issued (JavaScript uses it for "no
/// object").
pub struct Handles<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for Handles<T> {
    fn default() -> Self {
        Handles::new()
    }
}

impl<T> Handles<T> {
    pub fn new() -> Handles<T> {
        // slot 0 is reserved: index 0 with generation 0 would be handle 0
        Handles { slots: vec![Slot { generation: 0, value: None }], free: Vec::new() }
    }

    /// Stores `value`; `None` when the table is full.
    pub fn insert(&mut self, value: T) -> Option<u32> {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                if self.slots.len() > MAX_HANDLES {
                    return None;
                }
                self.slots.push(Slot { generation: 0, value: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.value = Some(value);
        // (`|` and `^` are the same here: the fields do not overlap)
        Some((slot.generation << INDEX_BITS) | index)
    }

    fn slot(&self, handle: u32) -> Option<&Slot<T>> {
        let slot = self.slots.get((handle & INDEX_MASK) as usize)?;
        (slot.generation == handle >> INDEX_BITS && slot.value.is_some()).then_some(slot)
    }

    pub fn get(&self, handle: u32) -> Option<&T> {
        self.slot(handle)?.value.as_ref()
    }

    pub fn get_mut(&mut self, handle: u32) -> Option<&mut T> {
        self.slot(handle)?;
        self.slots[(handle & INDEX_MASK) as usize].value.as_mut()
    }

    /// Removes the object; its handle becomes invalid.
    pub fn remove(&mut self, handle: u32) -> Option<T> {
        self.slot(handle)?;
        let index = handle & INDEX_MASK;
        let slot = &mut self.slots[index as usize];
        slot.generation = (slot.generation + 1) & GEN_MASK;
        self.free.push(index);
        slot.value.take()
    }

    /// The live objects, in slot order.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().filter_map(|s| s.value.as_mut())
    }

    /// The number of live objects.
    pub fn len(&self) -> usize {
        self.slots.len() - 1 - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_checked_and_reused_with_a_new_generation() {
        let mut h = Handles::new();
        assert!(h.is_empty());
        let a = h.insert("a").unwrap();
        assert!(!h.is_empty());
        let b = h.insert("b").unwrap();
        assert_ne!(a, 0);
        assert_ne!(a, b);
        assert_eq!(h.len(), 2);
        assert_eq!(h.get(a), Some(&"a"));
        assert_eq!(h.get(0), None);
        assert_eq!(h.remove(a), Some("a"));
        assert_eq!(h.get(a), None);
        assert_eq!(h.remove(a), None);
        let c = h.insert("c").unwrap();
        assert_eq!(c & INDEX_MASK, a & INDEX_MASK);
        assert_ne!(c, a);
        assert_eq!(h.get(a), None);
        *h.get_mut(c).unwrap() = "C";
        assert_eq!(h.get(c), Some(&"C"));
        assert!(h.get_mut(a).is_none());
        assert_eq!(h.len(), 2);
        assert_eq!(h.values_mut().map(|v| *v).collect::<Vec<_>>(), ["C", "b"]);
        assert_eq!(h.get(b | (1 << INDEX_BITS)), None);
        assert_eq!(h.get(12345), None);
    }

    #[test]
    fn generations_wrap_after_4096_uses_of_a_slot() {
        let mut h = Handles::new();
        let first = h.insert(0).unwrap();
        h.remove(first);
        for i in 1..4096 {
            let h_i = h.insert(i).unwrap();
            assert_eq!(h_i, (i << 20) | 1);
            h.remove(h_i);
        }
        assert_eq!(h.insert(7).unwrap(), first);
    }

    #[test]
    fn the_table_holds_max_handles_objects() {
        let mut h = Handles::new();
        for i in 1..=MAX_HANDLES as u32 {
            assert_eq!(h.insert(()), Some(i));
        }
        assert_eq!(h.len(), MAX_HANDLES);
        assert_eq!(MAX_HANDLES, (1 << 20) - 1);
        assert_eq!(h.insert(()), None);
        h.remove(5);
        assert_eq!(h.insert(()), Some((1 << 20) | 5));
    }
}
