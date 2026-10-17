//! Reading the code as it changed: how the files of the project on screen
//! differ from a base version of it (a git revision).

/// How a file differs between the two versions compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileStatus {
    Added,
    Removed,
    Modified,
    Renamed,
}

/// A file the change touched: its path in the head (or, removed, in the base),
/// and for a renamed file the path it had in the base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub status: FileStatus,
    pub previous: Option<String>,
}
