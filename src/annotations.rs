//! RenderDoc's version-one JSON notes and event bookmarks.
use serde_json::{Value, json};

pub fn set_comment(document: &Value, comment: &str) -> Result<Value, String> {
    let mut updated = document.clone();
    let object = updated
        .as_object_mut()
        .ok_or("Notes must be a JSON object")?;
    object.insert("comments".into(), Value::String(comment.replace('\r', "")));
    Ok(updated)
}

pub fn bookmarks(document: &Value) -> Result<Vec<(u32, String)>, String> {
    let object = document
        .as_object()
        .ok_or("Bookmarks must be a JSON object")?;
    let Some(value) = object.get("Bookmarks") else {
        return Ok(Vec::new());
    };
    let list = value.as_array().ok_or("Bookmarks must be an array")?;
    list.iter()
        .map(|item| {
            let event = item
                .get("eventId")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n != 0)
                .ok_or("Invalid bookmark event")?;
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .ok_or("Invalid bookmark text")?;
            Ok((event, text.to_owned()))
        })
        .collect()
}

pub fn set_bookmark(document: &Value, event: u32, text: Option<&str>) -> Result<Value, String> {
    if event == 0 {
        return Err("Bookmark event must be nonzero".into());
    }
    bookmarks(document)?;
    let mut updated = document.clone();
    let object = updated
        .as_object_mut()
        .ok_or("Bookmarks must be a JSON object")?;
    let list = object
        .entry("Bookmarks")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("Bookmarks must be an array")?;
    if let Some(index) = list
        .iter()
        .position(|item| item.get("eventId").and_then(Value::as_u64) == Some(u64::from(event)))
    {
        if let Some(text) = text {
            list[index]
                .as_object_mut()
                .ok_or("Invalid bookmark")?
                .insert("text".into(), Value::String(text.into()));
        } else {
            list.remove(index);
        }
    } else if let Some(text) = text {
        list.push(json!({"eventId":event,"text":text}));
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserve_unrelated_fields_and_reject_invalid_bookmarks() {
        let notes = json!({"comments":"old","plugin":{"v":1}});
        assert_eq!(
            set_comment(&notes, "中\r\n文").unwrap(),
            json!({"comments":"中\n文","plugin":{"v":1}})
        );
        let original = json!({"plugin":1,"Bookmarks":[{"eventId":7,"text":"old","extra":true}]});
        let changed = set_bookmark(&original, 7, Some("new")).unwrap();
        assert_eq!(changed["Bookmarks"][0]["extra"], true);
        assert_eq!(bookmarks(&changed).unwrap(), vec![(7, "new".into())]);
        assert_eq!(
            set_bookmark(&changed, 7, None).unwrap()["Bookmarks"],
            json!([])
        );
        assert!(set_bookmark(&json!({"Bookmarks":"invalid"}), 1, Some("x")).is_err());
    }
}
