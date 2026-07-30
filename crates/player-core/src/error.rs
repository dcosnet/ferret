use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("mpv error: {0}")]
    Mpv(#[from] mpv_bindings::MpvError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("engine not running")]
    EngineNotRunning,
    #[error("engine already stopped")]
    EngineStopped,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("engine thread panicked")]
    EnginePanic,
    #[error("{0}")]
    Other(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
