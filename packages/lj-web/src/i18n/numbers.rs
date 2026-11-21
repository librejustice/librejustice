/// How a number and a count are written.
pub struct NumbersUi {
    /// Between groups of three digits.
    pub thousands: char,
    /// From an amount already grouped.
    pub euros: fn(&str) -> String,
    /// A search's result count, from its total.
    pub results: fn(i64) -> String,
    pub decisions: fn(i64) -> String,
}
