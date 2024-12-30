pub mod preemptive;
pub mod sequential;

pub(super) trait BorrowPinMut<'a, T: ?Sized> {
    fn borrow_pin_mut(&mut self) -> &mut core::pin::Pin<&'a mut T>;
}
