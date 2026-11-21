use crate::i18n::DatesUi;

pub const DATES: DatesUi = DatesUi {
    months: [
        "Januari",
        "Februari",
        "Maart",
        "April",
        "Mei",
        "Juni",
        "Juli",
        "Augustus",
        "September",
        "Oktober",
        "November",
        "December",
    ],
    months_short: [
        "jan.", "feb.", "mrt.", "apr.", "mei", "jun.", "jul.", "aug.", "sep.", "okt.", "nov.",
        "dec.",
    ],
    weekdays: ["ma", "di", "wo", "do", "vr", "za", "zo"],
    from: "Van",
    to: "Tot",
    date: "Datum",
    month: "Maand",
    year: "Jaar",
    date_format: "dd/mm/jjjj",
    typed_label,
    clear,
    undated: "\u{2014}",
    short_date,
    long_date,
    numeric_date,
};

const MONTHS_IN_DATE: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];

fn typed_label(label: &str) -> String {
    format!("{label} (dag/maand/jaar)")
}

fn clear(label: &str) -> String {
    format!("{label} wissen")
}

fn short_date(year: i32, month: u32, day: u32) -> String {
    format!("{day} {} {year}", DATES.months_short[month as usize - 1])
}

fn long_date(year: i32, month: u32, day: u32) -> String {
    format!("{day} {} {year}", MONTHS_IN_DATE[month as usize - 1])
}

fn numeric_date(year: &str, month: &str, day: &str) -> String {
    format!("{day}/{month}/{year}")
}
