//! Manager error type. Ports `fds/manager/error.go` (messages verbatim).

use thiserror::Error;

use crate::error::FdsError;

#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("PartSize can not be smaller than 1")]
    PartSizeSmallerThanOne,
    #[error("Concurrency can not be smaller than 1")]
    ConcurrencySmallerThanOne,
    #[error("Does not support (bytes=i-j,m-n) format, only support (bytes=i-j)")]
    RangeFormat,
    #[error("BucketName or ObjectName is not matching")]
    BucketOrObjectNotMatching,
    #[error("MD5 is not matching")]
    Md5NotMatching,
    #[error("Object state is not matching")]
    ObjectStateNotMatching,
    #[error("Range is not matching")]
    RangeNotMatching,
    #[error("File is not found")]
    FileNotFound,
    #[error("Too many upload parts, increase PartSize please")]
    TooManyUploadParts,
    #[error(transparent)]
    Fds(#[from] FdsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
