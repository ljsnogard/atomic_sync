use core::fmt;

#[derive(Clone, Copy, Debug)]
pub enum SpinningRwLockError {
    Cancelled,
    Poisoned,
    Retry,
}

impl fmt::Display for SpinningRwLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpinningRwLockError::Cancelled => write!(f, "SpinningRwLockError::Cancelled"),
            SpinningRwLockError::Poisoned => write!(f, "SpinningRwLockError::Poisoned"),
            SpinningRwLockError::Retry => write!(f, "SpinningRwLockError::Retry"),
        }
    }
}

impl core::error::Error for SpinningRwLockError
{}
