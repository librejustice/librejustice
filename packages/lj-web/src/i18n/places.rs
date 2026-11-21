use super::Lang;

/// A place's name in `lang` from its code, `CH-VD` or `AT`, else `label`, the API's.
pub fn place_name(lang: Lang, code: &str, label: &str) -> String {
    let shell = &lang.ui().shell;
    (shell.place_name)(code)
        .or_else(|| (shell.state_name)(&code.to_ascii_lowercase()))
        .map_or_else(|| label.to_string(), str::to_string)
}

/// Every place of an article's applicability, named in `lang`.
pub fn relabel_applicability(lang: Lang, applicability: &mut lj_dtos::Applicability) {
    let lj_dtos::Applicability {
        reader,
        places,
        competing,
        elsewhere,
        readable,
        ..
    } = applicability;
    let states = competing.iter_mut().chain(elsewhere.iter_mut());
    for place in std::iter::once(reader)
        .chain(places.iter_mut())
        .chain(states.flat_map(|state| state.places.iter_mut()))
        .chain(readable.iter_mut().map(|r| &mut r.place))
    {
        place.label = place_name(lang, &place.uid, &place.label);
    }
}

/// A cantonal act's stored title, `Finanzausgleichsgesetz (Zurich), LS 132.1`, its canton named
/// in `lang`.
pub fn relabel_title(lang: Lang, title: &mut String) {
    let Some((code, at)) = lj_vocab::cantons::titled_canton(title) else {
        return;
    };
    if let Some(name) = (lang.ui().shell.place_name)(&format!("CH-{code}")) {
        title.replace_range(at, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cantonal_title_renames_its_canton_and_keeps_its_own_brackets() {
        let stored =
            "Gesetz über Lebensmittel (Lebensmittel- und Veterinärgesetz, LVG) (Nidwald), NG 717.1";
        let mut title = stored.to_string();
        relabel_title(Lang::De, &mut title);
        assert_eq!(
            title,
            "Gesetz über Lebensmittel (Lebensmittel- und Veterinärgesetz, LVG) (Nidwalden), NG 717.1"
        );
        let mut french = stored.to_string();
        relabel_title(Lang::Fr, &mut french);
        assert_eq!(french, stored);
        let mut own = "Vereinbarung mit Basel (Bâle-Ville)".to_string();
        relabel_title(Lang::De, &mut own);
        assert_eq!(own, "Vereinbarung mit Basel (Bâle-Ville)");
    }

    #[test]
    fn a_french_place_line_puts_the_place_s_own_preposition_first() {
        let located = Lang::Fr.ui().shell.located;
        assert_eq!(located("CH", "Suisse"), "en Suisse");
        assert_eq!(located("SN", "Sénégal"), "au Sénégal");
        assert_eq!(located("MC", "Monaco"), "à Monaco");
        assert_eq!(
            located("CH-BS", "Bâle-Ville"),
            "dans le canton de Bâle-Ville"
        );
        assert_eq!(located("CH-VS", "Valais"), "dans le canton du Valais");
        assert_eq!(
            (Lang::De.ui().shell.located)("CH-BS", "Basel-Stadt"),
            "Basel-Stadt"
        );
    }
}
