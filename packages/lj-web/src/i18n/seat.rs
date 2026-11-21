use lj_dtos::{SeatChamber, SeatContainer, SeatJoined, SeatParts};

/// The words a language names a seat's parts with, as they read inside a title.
pub struct SeatWords {
    pub container: fn(SeatContainer) -> String,
    /// A chamber, with the matters it judges when it has some.
    pub chamber: fn(SeatChamber, Option<&str>) -> String,
    /// A room no number designates, by the matters it judges.
    pub room: fn(&str) -> String,
    /// A matter, by its key: `CIVILE`.
    pub matter: fn(&str) -> Option<&'static str>,
    pub and: &'static str,
    pub section: fn(u32) -> String,
    pub letter: fn(char) -> String,
    pub joined: fn(&SeatJoined) -> String,
    /// A formation, solemn or not, by its key: `GRANDE_CHAMBRE`, `A_TROIS`.
    pub formation: fn(&str) -> Option<&'static str>,
    /// An office, by its key: `JLD`.
    pub office: fn(&str) -> Option<&'static str>,
    /// A table's label as it reads inside a title.
    pub mid: fn(&str) -> String,
}

/// The room, then its formation and office; `None` when `words` lacks a part's name.
pub fn compose(words: &SeatWords, parts: &SeatParts) -> Option<String> {
    let matters = match parts.matters.as_slice() {
        [] => None,
        keys => Some(
            keys.iter()
                .map(|key| (words.matter)(key))
                .collect::<Option<Vec<_>>>()?
                .join(words.and),
        ),
    };
    let mut room: Vec<String> = parts.container.map(words.container).into_iter().collect();
    match (parts.chamber, parts.solemn.as_deref()) {
        (Some(chamber), _) => room.push((words.chamber)(chamber, matters.as_deref())),
        (None, Some(solemn)) => room.push((words.mid)((words.formation)(solemn)?)),
        (None, None) => room.extend(matters.as_deref().map(words.room)),
    }
    room.extend(parts.section.map(words.section));
    room.extend(parts.joined.as_ref().map(words.joined));
    match (parts.letter, parts.chamber, parts.section) {
        (Some(letter), None, Some(_)) => {
            if let Some(section) = room.last_mut() {
                section.push(' ');
                section.push(letter);
            }
        }
        (Some(letter), _, _) => room.push((words.letter)(letter)),
        (None, _, _) => {}
    }
    let mut segments: Vec<String> = Vec::new();
    if !room.is_empty() {
        segments.push(room.join(", "));
    }
    if let Some(key) = parts.formation.as_deref() {
        segments.push((words.mid)((words.formation)(key)?));
    }
    if let Some(key) = parts.office.as_deref() {
        segments.push((words.mid)((words.office)(key)?));
    }
    (!segments.is_empty()).then(|| segments.join(" · "))
}

/// The first letter lowered, unless the word is an acronym.
pub fn lower_first(label: &str) -> String {
    let mut chars = label.chars();
    match (chars.next(), chars.next()) {
        (Some(first), second) if !second.is_some_and(char::is_uppercase) => {
            first.to_lowercase().collect::<String>() + &label[first.len_utf8()..]
        }
        _ => label.to_string(),
    }
}

/// Room numbers sitting together, in order: `3/5`.
pub fn numbers(nums: &[u32]) -> String {
    let mut nums = nums.to_vec();
    nums.sort_unstable();
    nums.dedup();
    nums.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use crate::i18n::Lang;
    use lj_dtos::{SeatChamber, SeatContainer, SeatParts};

    #[test]
    fn a_seat_is_named_by_its_parts() {
        let cases = [
            SeatParts {
                container: Some(SeatContainer::Pole(4)),
                chamber: Some(SeatChamber::Number(1)),
                ..Default::default()
            },
            SeatParts {
                chamber: Some(SeatChamber::Number(3)),
                letter: Some('A'),
                ..Default::default()
            },
            SeatParts {
                matters: vec!["SOCIALE".to_string()],
                formation: Some("JUGE_UNIQUE".to_string()),
                ..Default::default()
            },
            SeatParts {
                chamber: Some(SeatChamber::Number(2)),
                matters: vec!["CIVILE".to_string(), "COMMERCIALE".to_string()],
                office: Some("JLD".to_string()),
                ..Default::default()
            },
            SeatParts {
                solemn: Some("GRANDE_CHAMBRE".to_string()),
                ..Default::default()
            },
        ];
        let named = |lang: Lang| {
            cases
                .iter()
                .map(|parts| (lang.ui().referential.seat)(parts))
                .collect::<Vec<_>>()
        };
        assert_eq!(named(Lang::Fr), [None, None, None, None, None]);
        assert_eq!(
            named(Lang::Nl),
            [
                Some("afdeling 4, 1e kamer".to_string()),
                Some("3e kamer, sectie A".to_string()),
                Some("kamer voor sociale zaken · alleenzetelend rechter".to_string()),
                Some(
                    "2e kamer voor burgerlijke zaken en handelszaken · rechter voor vrijheden en hechtenis"
                        .to_string()
                ),
                Some("grote kamer".to_string()),
            ]
        );
        assert_eq!(
            named(Lang::De),
            [
                Some("Abteilung 4, 1. Kammer".to_string()),
                Some("3. Kammer, Sektion A".to_string()),
                Some("Kammer für Sozialsachen · Einzelrichter".to_string()),
                Some(
                    "2. Kammer für Zivilsachen und Handelssachen · Zwangsmassnahmenrichter"
                        .to_string()
                ),
                Some("Grosse Kammer".to_string()),
            ]
        );
    }
}
