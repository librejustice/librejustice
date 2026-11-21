use crate::helpers::group_thousands;
use crate::i18n::NumbersUi;

pub const NUMBERS: NumbersUi = NumbersUi {
    thousands: '\u{2019}',
    euros,
    results,
    decisions,
};

fn euros(amount: &str) -> String {
    format!("EUR {amount}")
}

fn results(total: i64) -> String {
    match total {
        0 => "Keine Treffer".to_string(),
        1 => "1 Treffer".to_string(),
        n => format!("{} Treffer", group_thousands(&NUMBERS, n)),
    }
}

fn decisions(n: i64) -> String {
    match n {
        1 => "1 Entscheid".to_string(),
        n => format!("{} Entscheide", group_thousands(&NUMBERS, n)),
    }
}
