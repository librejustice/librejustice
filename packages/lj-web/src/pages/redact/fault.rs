//! What stopped the workbench, as a value the catalogue words where it is shown.

/// Every fault is ours but [`Fault::Said`], which carries the words of the redaction
/// module, a text reader or the host, passed on as they came.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    Said(String),
    /// The module threw something that is no message.
    Unknown,
    NotExposed(String),
    NoWindow,
    NoDocument,
    LoaderMissing,
    LoaderSilent,
    ModuleUnloaded(Box<Fault>),
    NoPromise,
    NoPixels,
    NoBytes,
    NoPageImage,
    BadMatrix,
    NoPageDrawing,
    NoFontList,
    FontWithoutFamily,
    FontWithoutSource(String),
    FileUnread,
    FileUnassembled,
    Unsupported,
    ImageUndecoded,
    ImageUndrawn,
    ImageUnencoded,
    FontNotReceived(String),
    FontUnread,
    FilesUnloaded,
    /// A file of the tool, and the status it was answered with.
    NotServed(String, u16),
    FontUndeclared(String),
    NothingOpen,
    NoImageOpen,
    No2d,
    PageUnread,
    PageUndrawn,
    PageUnconverted,
    /// A folder of the tool whose index is no text.
    NotText(String),
    NoCanvas,
    HostSilent,
    HostRefused,
    HostLacks(String),
    HostGaveNoAddress,
    HostKeepsNoFile,
    HostKeptNoFile,
    MessageNotSent,
}

impl Fault {
    pub fn said(error: impl std::fmt::Display) -> Self {
        Fault::Said(error.to_string())
    }
}
