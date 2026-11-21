/// Month and day names, and the date fields that read what a reader types.
pub struct DatesUi {
    /// January first, as a calendar heads its month; a typed date is read against them too.
    pub months: [&'static str; 12],
    /// January first, as a short date writes them; a typed date is read against them too.
    pub months_short: [&'static str; 12],
    /// Monday first.
    pub weekdays: [&'static str; 7],
    pub from: &'static str,
    pub to: &'static str,
    pub date: &'static str,
    pub month: &'static str,
    pub year: &'static str,
    pub date_format: &'static str,
    /// A date field's accessible name, from its label.
    pub typed_label: fn(&str) -> String,
    /// The button that empties a date field, from its label.
    pub clear: fn(&str) -> String,
    /// What stands where a date is missing.
    pub undated: &'static str,
    /// A date with its month shortened, from its year, month and day.
    pub short_date: fn(i32, u32, u32) -> String,
    /// A date written in full, from its year, month and day.
    pub long_date: fn(i32, u32, u32) -> String,
    /// A date in figures, from its year, month and day as written: two digits each but the year.
    pub numeric_date: fn(&str, &str, &str) -> String,
}
