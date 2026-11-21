use super::Run;

/// The legal pages: terms of use, privacy policy, legal notice.
pub struct LegalUi {
    pub terms: LegalPage,
    pub privacy: LegalPage,
    pub notice: LegalPage,
}

pub struct LegalPage {
    pub title: &'static str,
    pub heading: &'static str,
    pub in_force: &'static str,
    pub sections: &'static [LegalSection],
}

pub struct LegalSection {
    pub title: &'static str,
    pub blocks: &'static [Block],
}

pub enum Block {
    Para(&'static [Run]),
    List(&'static [&'static [Run]]),
    /// A postal or contact address, one line per entry.
    Address(&'static [&'static [Run]]),
}
