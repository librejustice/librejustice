use crate::helpers::group_thousands;
use crate::i18n::NumbersUi;

pub const NUMBERS: NumbersUi = NumbersUi {
    thousands: '\u{202F}',
    euros,
    results,
    decisions,
};

fn euros(amount: &str) -> String {
    format!("{amount} €")
}

fn results(total: i64) -> String {
    match total {
        0 => "Aucun résultat".to_string(),
        1 => "1 résultat".to_string(),
        n => format!("{} résultats", group_thousands(&NUMBERS, n)),
    }
}

fn decisions(n: i64) -> String {
    match n {
        1 => "1 décision".to_string(),
        n => format!("{} décisions", group_thousands(&NUMBERS, n)),
    }
}
