use crate::helpers::group_thousands;
use crate::i18n::NumbersUi;

pub const NUMBERS: NumbersUi = NumbersUi {
    thousands: '.',
    euros,
    results,
    decisions,
};

fn euros(amount: &str) -> String {
    format!("€ {amount}")
}

fn results(total: i64) -> String {
    match total {
        0 => "Geen resultaten".to_string(),
        1 => "1 resultaat".to_string(),
        n => format!("{} resultaten", group_thousands(&NUMBERS, n)),
    }
}

fn decisions(n: i64) -> String {
    match n {
        1 => "1 beslissing".to_string(),
        n => format!("{} beslissingen", group_thousands(&NUMBERS, n)),
    }
}
