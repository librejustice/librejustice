use crate::i18n::DatesUi;

pub const DATES: DatesUi = DatesUi {
    months: [
        "Janvier",
        "Février",
        "Mars",
        "Avril",
        "Mai",
        "Juin",
        "Juillet",
        "Août",
        "Septembre",
        "Octobre",
        "Novembre",
        "Décembre",
    ],
    months_short: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    weekdays: ["lu", "ma", "me", "je", "ve", "sa", "di"],
    from: "Du",
    to: "Au",
    date: "Date",
    month: "Mois",
    year: "Année",
    date_format: "jj/mm/aaaa",
    typed_label,
    clear,
    undated: "\u{2014}",
    short_date,
    long_date,
    numeric_date,
};

const MONTHS_IN_DATE: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

fn typed_label(label: &str) -> String {
    format!("{label} (jour/mois/année)")
}

fn clear(label: &str) -> String {
    format!("Effacer {label}")
}

fn short_date(year: i32, month: u32, day: u32) -> String {
    format!(
        "{} {} {year}",
        ordinal(day),
        DATES.months_short[month as usize - 1]
    )
}

fn long_date(year: i32, month: u32, day: u32) -> String {
    format!(
        "{} {} {year}",
        ordinal(day),
        MONTHS_IN_DATE[month as usize - 1]
    )
}

fn ordinal(day: u32) -> String {
    match day {
        1 => "1er".to_string(),
        day => day.to_string(),
    }
}

fn numeric_date(year: &str, month: &str, day: &str) -> String {
    format!("{day}/{month}/{year}")
}
