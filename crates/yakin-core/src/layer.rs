//! Config files laid over each other: a baseline, then drop-ins in file name order.

use anyhow::anyhow;
use serde::Deserialize;
use toml::Table;
use toml::Value;

use crate::Result;
use crate::profile::ConfigToml;

/// Lay `over` onto `base`: tables merge key by key, any other value (arrays too) replaces.
pub fn merge(
    base: &mut Table,
    over: Table,
) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(base)), Value::Table(over)) => merge(base, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// The smallest table that [`merge`]s `base` into `over`, for the keys `over` has: equal values
/// are left out, tables are compared key by key and dropped when nothing in them differs.
pub fn diff(
    base: &Table,
    over: &Table,
) -> Table {
    let mut out = Table::new();
    for (key, value) in over {
        match (base.get(key), value) {
            (Some(Value::Table(base)), Value::Table(over)) => {
                let inner = diff(base, over);
                if !inner.is_empty() {
                    out.insert(key.clone(), Value::Table(inner));
                }
            }
            (Some(base), value) if base == value => {}
            (_, value) => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

pub fn parse(text: &str) -> Result<Table> {
    Ok(text.parse::<Table>()?)
}

/// `toml` as a table. Its floats are `f32`; each is written the shortest way that reads back
/// the same, so `0.3` stays `0.3` and compares equal to the file it came from.
pub fn to_table(toml: &ConfigToml) -> Result<Table> {
    match Value::try_from(toml)? {
        Value::Table(mut table) => {
            table.iter_mut().for_each(|(_, value)| shorten(value));
            Ok(table)
        }
        _ => Err(anyhow!("Config is not a table")),
    }
}

fn shorten(value: &mut Value) {
    match value {
        Value::Float(float) => {
            *float = (*float as f32).to_string().parse().unwrap_or(*float);
        }
        Value::Table(table) => table.iter_mut().for_each(|(_, value)| shorten(value)),
        Value::Array(array) => array.iter_mut().for_each(shorten),
        _ => {}
    }
}

pub fn from_table(table: Table) -> Result<ConfigToml> {
    Ok(ConfigToml::deserialize(table)?)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::profile::BindingToml;

    fn merged(texts: &[&str]) -> Table {
        let mut base = Table::new();
        for text in texts {
            merge(&mut base, parse(text).unwrap());
        }
        base
    }

    #[test]
    fn test_merge_nested() {
        let table = merged(&[
            "[profile.desktop.buttons]\na = \"mouse:left\"\nb = \"key:KEY_ESC\"",
            "[profile.desktop.buttons]\na = \"mouse:right\"",
        ]);
        assert_eq!(
            table,
            parse("[profile.desktop.buttons]\na = \"mouse:right\"\nb = \"key:KEY_ESC\"").unwrap()
        );
    }

    #[test]
    fn test_merge_replaces_arrays() {
        let table = merged(&[
            "[[profile.x.chord]]\nhold = \"view\"\na = \"key:KEY_A\"\n\
             [[profile.x.chord]]\nhold = \"menu\"\nb = \"key:KEY_B\"",
            "[[profile.x.chord]]\nhold = \"lb\"\nx = \"key:KEY_X\"",
        ]);
        assert_eq!(
            table,
            parse("[[profile.x.chord]]\nhold = \"lb\"\nx = \"key:KEY_X\"").unwrap()
        );
    }

    #[test]
    fn test_merge_adds_profile() {
        let table = merged(&[
            "[daemon]\nactive = \"a\"\n[profile.a.buttons]\na = \"pad:south\"",
            "[daemon]\nactive = \"b\"\n[profile.b.buttons]\nb = \"pad:east\"",
        ]);
        let toml = from_table(table).unwrap();
        assert_eq!(toml.daemon.active.as_deref(), Some("b"));
        assert_eq!(toml.profile.len(), 2);
    }

    #[test]
    fn test_merge_in_order() {
        let base = "[profile.p.buttons]\na = \"pad:south\"";
        let ten = "[profile.p.buttons]\na = \"pad:east\"";
        let twenty = "[profile.p.buttons]\na = \"pad:west\"";
        let table = merged(&[base, ten, twenty]);
        assert_eq!(
            table["profile"]["p"]["buttons"]["a"].as_str(),
            Some("pad:west")
        );
    }

    #[test]
    fn test_diff() {
        let base = parse(
            "[profile.p.buttons]\na = \"mouse:left\"\nx = \"pad:west\"\n\
             [profile.same.buttons]\na = \"pad:south\"",
        )
        .unwrap();
        let over = parse(
            "[profile.p.buttons]\na = \"mouse:left\"\nx = \"key:KEY_X\"\n\
             [profile.same.buttons]\na = \"pad:south\"\n\
             [profile.new.buttons]\na = \"pad:south\"",
        )
        .unwrap();
        let out = diff(&base, &over);
        assert_eq!(
            out,
            parse(
                "[profile.p.buttons]\nx = \"key:KEY_X\"\n\
                 [profile.new.buttons]\na = \"pad:south\""
            )
            .unwrap()
        );
        let mut back = base.clone();
        merge(&mut back, out);
        assert_eq!(back, over);
    }

    #[test]
    fn test_diff_activators() {
        let base = parse(
            "[profile.p.buttons]\na = \"pad:south\"\nb = { press = \"pad:east\", long = \"key:KEY_ESC\" }\n\
             [[profile.p.chord]]\nhold = \"view\"\nx = { press = \"key:KEY_X\", turbo_ms = 100 }",
        )
        .unwrap();
        let mut edited = from_table(base.clone()).unwrap();
        let buttons = &mut edited.profile.get_mut("p").unwrap().buttons;
        let toml = "a = { press = \"pad:south\", double = \"mouse:middle\", double_ms = 200 }";
        let table: Table = toml.parse().unwrap();
        let a = BindingToml::deserialize(table["a"].clone()).unwrap();
        buttons.insert("a".to_string(), a);
        let over = to_table(&edited).unwrap();
        let out = diff(&base, &over);
        assert_eq!(
            out,
            parse(
                "[profile.p.buttons]\n\
                 a = { press = \"pad:south\", double = \"mouse:middle\", double_ms = 200 }"
            )
            .unwrap()
        );
        let mut back = base.clone();
        merge(&mut back, out);
        assert_eq!(from_table(back).unwrap(), edited);
    }

    #[test]
    fn test_to_table_short_floats() {
        let toml = ConfigToml::parse("[profile.p.mouse]\nspeed = 0.3\n").unwrap();
        let table = to_table(&toml).unwrap();
        assert_eq!(table, parse("[profile.p.mouse]\nspeed = 0.3\n").unwrap());
    }
}
