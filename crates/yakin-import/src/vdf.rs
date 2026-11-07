//! Case-insensitive lookups over a parsed KeyValues tree. Steam's files mix `Full_Press` and
//! `full_press`, `button_B` and `button_b`.

pub use keyvalues_parser::Obj;
pub use keyvalues_parser::Value;

/// Every value under `key`, in file order for repeated keys.
pub fn all<'a>(
    obj: &'a Obj<'a>,
    key: &str,
) -> impl Iterator<Item = &'a Value<'a>> {
    obj.iter()
        .filter(move |(known, _)| known.eq_ignore_ascii_case(key))
        .flat_map(|(_, values)| values.iter())
}

pub fn obj<'a>(
    obj: &'a Obj<'a>,
    key: &str,
) -> Option<&'a Obj<'a>> {
    all(obj, key).find_map(Value::get_obj)
}

pub fn text<'a>(
    obj: &'a Obj<'a>,
    key: &str,
) -> Option<&'a str> {
    all(obj, key).find_map(Value::get_str)
}

/// Every `(key, string)` pair, nested objects left out.
pub fn pairs<'a>(obj: &'a Obj<'a>) -> impl Iterator<Item = (&'a str, &'a str)> {
    obj.iter().flat_map(|(key, values)| {
        values
            .iter()
            .filter_map(Value::get_str)
            .map(move |value| (key.as_ref(), value))
    })
}
