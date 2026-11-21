use crate::i18n::ReferentialUi;

pub const REFERENTIAL: ReferentialUi = ReferentialUi {
    solution: |_| None,
    jurisdiction_type: |_| None,
    legal_domain: |_| None,
    formation: |_| None,
    publication: |_| None,
    nature: |_| None,
    norm_source: |_| None,
    norm_scope: |_| None,
    seat: |_| None,
};
