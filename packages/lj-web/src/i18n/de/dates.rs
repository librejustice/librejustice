use crate::i18n::DatesUi;

pub const DATES: DatesUi = DatesUi {
    months: [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    months_short: [
        "Jan.", "Febr.", "März", "April", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.",
        "Dez.",
    ],
    weekdays: ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
    from: "Von",
    to: "Bis",
    date: "Datum",
    month: "Monat",
    year: "Jahr",
    date_format: "TT.MM.JJJJ",
    typed_label,
    clear,
    undated: "\u{2014}",
    short_date,
    long_date,
    numeric_date,
};

fn typed_label(label: &str) -> String {
    format!("{label} (TT.MM.JJJJ)")
}

fn clear(label: &str) -> String {
    format!("{label} löschen")
}

fn short_date(year: i32, month: u32, day: u32) -> String {
    format!("{day}. {} {year}", DATES.months_short[month as usize - 1])
}

fn long_date(year: i32, month: u32, day: u32) -> String {
    format!("{day}. {} {year}", DATES.months[month as usize - 1])
}

fn numeric_date(year: &str, month: &str, day: &str) -> String {
    format!("{day}.{month}.{year}")
}
