//! What an import used and what it left out, summed over files for `import --report`.

use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Mode,
    Verb,
    Activator,
    Setting,
    /// Why a whole group was skipped.
    Reason,
}
impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Mode,
        Kind::Verb,
        Kind::Activator,
        Kind::Setting,
        Kind::Reason,
    ];

    fn name(self) -> &'static str {
        match self {
            Kind::Mode => "mode",
            Kind::Verb => "verb",
            Kind::Activator => "activator",
            Kind::Setting => "setting",
            Kind::Reason => "group skip",
        }
    }
}

/// Executed and skipped counts per kind and name.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    counts: BTreeMap<(Kind, String), [u32; 2]>,
}
impl Tally {
    pub fn executed(
        &mut self,
        kind: Kind,
        name: &str,
    ) {
        self.count(kind, name, 0);
    }

    pub fn skipped(
        &mut self,
        kind: Kind,
        name: &str,
    ) {
        self.count(kind, name, 1);
    }

    fn count(
        &mut self,
        kind: Kind,
        name: &str,
        slot: usize,
    ) {
        // Steam writes both `Full_Press` and `full_press`
        let name = match kind {
            Kind::Activator => name.to_ascii_lowercase(),
            _ => name.to_string(),
        };
        self.counts.entry((kind, name)).or_default()[slot] += 1;
    }

    /// `[executed, skipped]` for one name.
    pub fn get(
        &self,
        kind: Kind,
        name: &str,
    ) -> [u32; 2] {
        self.counts
            .get(&(kind, name.to_string()))
            .copied()
            .unwrap_or_default()
    }

    pub fn merge(
        &mut self,
        other: &Tally,
    ) {
        for (key, [executed, skipped]) in &other.counts {
            let counts = self.counts.entry(key.clone()).or_default();
            counts[0] += executed;
            counts[1] += skipped;
        }
    }

    /// One table per kind, then the `top` most skipped names of any kind.
    pub fn table(
        &self,
        top: usize,
    ) -> String {
        let mut out = String::new();
        for kind in Kind::ALL {
            writeln!(
                out,
                "{:<10} {:<32} {:>6} {:>6} {:>6}",
                kind.name(),
                "",
                "seen",
                "exec",
                "skip"
            )
            .unwrap();
            let mut rows: Vec<_> = self
                .counts
                .iter()
                .filter(|((known, _), _)| *known == kind)
                .collect();
            rows.sort_by_key(|(_, [executed, skipped])| std::cmp::Reverse(executed + skipped));
            for ((_, name), [executed, skipped]) in rows {
                writeln!(
                    out,
                    "{:<10} {name:<32} {:>6} {executed:>6} {skipped:>6}",
                    "",
                    executed + skipped
                )
                .unwrap();
            }
            writeln!(out).unwrap();
        }
        let mut skips: Vec<_> = self
            .counts
            .iter()
            .filter(|(_, [_, skipped])| *skipped > 0)
            .collect();
        skips.sort_by_key(|(_, [_, skipped])| std::cmp::Reverse(*skipped));
        writeln!(out, "top skipped").unwrap();
        for ((kind, name), [_, skipped]) in skips.into_iter().take(top) {
            writeln!(out, "{skipped:>6} {} {name}", kind.name()).unwrap();
        }
        out
    }
}
