pub mod preemptive;
// pub mod sequential;

pub(super) trait TrShareMut<'a, T: ?Sized> {
    fn share_mut(&mut self) -> &'a mut T;
}
