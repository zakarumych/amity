use core::mem::MaybeUninit;

/// Safer version of `UnsafeSlot`.
struct Slot<T> {
    value: MaybeUninit<T>,
    next: usize,
}

impl<T> Slot<T> {
    fn vacant(&self) -> usize {
        debug_assert_ne!(self.next, usize::MAX);
        self.next
    }

    unsafe fn occupied_ref(&self) -> &T {
        self.value.assume_init_ref()
    }

    unsafe fn occupied_mut(&mut self) -> &mut T {
        self.value.assume_init_mut()
    }

    fn make_occupied(&mut self) -> &mut MaybeUninit<T> {
        self.next = usize::MAX;
        &mut self.value
    }

    unsafe fn occupied_drop(&mut self, next: usize) {
        debug_assert_ne!(next, usize::MAX);
        self.next = next;
        self.value.assume_init_drop();
    }

    unsafe fn occupied_read(&mut self, next: usize) -> T {
        debug_assert_ne!(next, usize::MAX);
        self.next = next;
        self.value.assume_init_read()
    }
}

/// Typed arena that can store values of a single type `T`.
pub struct Arena<T> {
    slots: Box<[Slot<T>]>,
    next: usize,
}

impl<T> Arena<T> {
    pub const fn new() -> Self {
        Arena {
            slots: Box::new([]),
            next: 0,
        }
    }

    pub fn insert(&mut self, value: T) -> usize {
        let index = self.next;
        if index < self.slots.len() {
            let slot = &mut self.slots[index];
            self.next = slot.vacant();
            *slot.make_occupied() = MaybeUninit::new(value);
        } else {
            self.slots.push(Slot {
                value: MaybeUninit::new(value),
                next: 0,
            });
            self.next = self.slots.len();
        }
        index
    }

    pub fn remove(&mut self, idx: usize) -> T {
        let slot = &mut self.slots[idx];
        let value = unsafe { slot.occupied_read(self.next) };
        self.next = idx;
        value
    }

    pub fn drop(&mut self, idx: usize) -> T {
        let slot = &mut self.slots[idx];
        let next = slot.vacant();
        unsafe { slot.occupied_drop(self.next) };
        self.next = next;
        unsafe { slot.occupied_read(self.next) }
    }

    pub fn get(&self, idx: usize) -> &T {
        unsafe { self.slots[idx].occupied_ref() }
    }

    pub fn get_mut(&mut self, idx: usize) -> &mut T {
        unsafe { self.slots[idx].occupied_mut() }
    }
}

fn not_vacant() -> ! {
    panic!("slot is not vacant")
}

fn not_occupied() -> ! {
    panic!("slot is not occupied")
}
