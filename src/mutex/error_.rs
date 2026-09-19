use core::fmt;

#[derive(Clone, Copy, Debug)]
pub enum SpinningMutexError {
    Cancelled,
    Poisoned,
    Retry,
}

impl fmt::Display for SpinningMutexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpinningMutexError::Cancelled => write!(f, "SpinningMutexError::Cancelled"),
            SpinningMutexError::Poisoned => write!(f, "SpinningMutexError::Poisoned"),
            SpinningMutexError::Retry => write!(f, "SpinningMutexError::Retry"),
        }
    }
}

impl core::error::Error for SpinningMutexError
{}
