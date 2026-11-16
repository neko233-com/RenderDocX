//! RenderDoc's version-one JSON notes, bookmarks and resource names.
use serde_json::{Value, json};

pub fn shader_editors(document: &Value) -> Result<&[Value], String> {
    let object = document
        .as_object()
        .ok_or("Shader edits must be a JSON object")?;
    match object.get("editors") {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| "Shader editors must be an array".into()),
    }
}

pub fn shader_editor<'a>(
    document: &'a Value,
    id: &str,
    stage: u32,
) -> Result<Option<&'a Value>, String> {
    let mut found = None;
    for editor in shader_editors(document)? {
        let object = editor.as_object().ok_or("Invalid shader editor")?;
        let editor_id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Shader editor has no resource ID")?;
        let editor_stage = object
            .get("stage")
            .and_then(Value::as_u64)
            .ok_or("Shader editor has no stage")?;
        if editor_id == id && editor_stage == u64::from(stage) {
            if found.is_some() {
                return Err("Multiple saved editors match this shader and stage".into());
            }
            found = Some(editor);
        }
    }
    Ok(found)
}

pub fn upsert_shader_editor(document: &Value, editor: &Value) -> Result<Value, String> {
    let object = editor.as_object().ok_or("Invalid shader editor")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Shader editor has no resource ID")?;
    let stage = object
        .get("stage")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n < 8)
        .ok_or("Invalid shader stage")?;
    let numeric_id = id
        .strip_prefix("ResourceId::")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v != 0)
        .ok_or("Invalid RenderDoc resource identifier")?;
    if id != format!("ResourceId::{numeric_id}") {
        return Err("Noncanonical RenderDoc resource identifier".into());
    }
    let files = object
        .get("files")
        .and_then(Value::as_array)
        .filter(|files| !files.is_empty())
        .ok_or("Shader editor needs source files")?;
    if files.iter().any(|file| {
        file.get("name").and_then(Value::as_str).is_none()
            || file.get("contents").and_then(Value::as_str).is_none()
    }) {
        return Err("Invalid shader editor file".into());
    }
    for key in ["entryPoint", "commandLine", "tool"] {
        if object.get(key).and_then(Value::as_str).is_none() {
            return Err(format!("Invalid shader editor {key}"));
        }
    }
    if object
        .get("encoding")
        .and_then(Value::as_u64)
        .is_none_or(|v| v >= 10)
        || object
            .get("flags")
            .and_then(Value::as_object)
            .is_none_or(|flags| flags.values().any(|value| !value.is_string()))
    {
        return Err("Invalid shader editor encoding or flags".into());
    }
    let existing = shader_editor(document, id, stage)?;
    let mut merged = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    merged.extend(object.clone());
    let mut updated = document.clone();
    let root = updated
        .as_object_mut()
        .ok_or("Shader edits must be a JSON object")?;
    let editors = root
        .entry("editors")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("Shader editors must be an array")?;
    if let Some(index) = editors.iter().position(|item| {
        item.get("id").and_then(Value::as_str) == Some(id)
            && item.get("stage").and_then(Value::as_u64) == Some(u64::from(stage))
    }) {
        editors[index] = Value::Object(merged);
    } else {
        editors.push(Value::Object(merged));
    }
    Ok(updated)
}

pub fn display_resource_name<'a>(
    resources: &'a [crate::replay::ResourceInfo],
    document: Option<&'a Value>,
    index: Option<usize>,
    fallback: &'a str,
) -> &'a str {
    index
        .and_then(|index| resources.get(index))
        .and_then(|resource| resource.rename_key.as_deref())
        .and_then(|key| document.and_then(|doc| resource_name(doc, key).ok().flatten()))
        .unwrap_or(fallback)
}

pub fn resource_name<'a>(document: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    let object = document
        .as_object()
        .ok_or("Resource names must be a JSON object")?;
    let Some(names) = object.get("CustomResourceNames") else {
        return Ok(None);
    };
    let names = names
        .as_object()
        .ok_or("CustomResourceNames must be an object")?;
    match names.get(key) {
        None => Ok(None),
        Some(name) => name
            .as_str()
            .map(Some)
            .ok_or_else(|| "Resource name must be a string".into()),
    }
}

pub fn set_resource_name(document: &Value, key: &str, name: &str) -> Result<Value, String> {
    let Some(id) = key
        .strip_prefix("ResourceId::")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|id| *id != 0)
    else {
        return Err("Invalid RenderDoc resource identifier".into());
    };
    if key != format!("ResourceId::{id}") {
        return Err("Noncanonical RenderDoc resource identifier".into());
    }
    resource_name(document, key)?;
    let mut updated = document.clone();
    let object = updated
        .as_object_mut()
        .ok_or("Resource names must be a JSON object")?;
    let names = object
        .entry("CustomResourceNames")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("CustomResourceNames must be an object")?;
    if name.is_empty() {
        names.remove(key);
    } else {
        names.insert(key.into(), Value::String(name.into()));
    }
    Ok(updated)
}

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

    #[test]
    fn rename_uses_renderdoc_keys_and_preserves_other_entries() {
        let original = json!({"extra":true,"CustomResourceNames":{"ResourceId::7":"old","ResourceId::8":"keep"}});
        let renamed = set_resource_name(&original, "ResourceId::7", "颜色").unwrap();
        assert_eq!(
            resource_name(&renamed, "ResourceId::7").unwrap(),
            Some("颜色")
        );
        assert_eq!(renamed["CustomResourceNames"]["ResourceId::8"], "keep");
        assert_eq!(renamed["extra"], true);
        assert_eq!(
            resource_name(
                &set_resource_name(&renamed, "ResourceId::7", "").unwrap(),
                "ResourceId::7"
            )
            .unwrap(),
            None
        );
        assert!(set_resource_name(&original, "ResourceId::0", "x").is_err());
        assert!(
            set_resource_name(&json!({"CustomResourceNames":[]}), "ResourceId::7", "x").is_err()
        );
    }

    #[test]
    fn display_names_follow_resource_identity_and_fallback() {
        let resource = |key: &str| crate::replay::ResourceInfo {
            name: "duplicate".into(),
            rename_key: Some(key.into()),
            resource_type: 0,
            autogenerated_name: false,
            initialisation_chunks: vec![],
            parents: vec![],
            derived: vec![],
        };
        let resources = [resource("ResourceId::1"), resource("ResourceId::2")];
        let names = json!({"CustomResourceNames":{"ResourceId::2":"custom"}});
        assert_eq!(
            display_resource_name(&resources, Some(&names), Some(1), "duplicate"),
            "custom"
        );
        assert_eq!(
            display_resource_name(&resources, Some(&names), Some(0), "duplicate"),
            "duplicate"
        );
        assert_eq!(
            display_resource_name(&resources, Some(&names), Some(20), "fallback"),
            "fallback"
        );
        assert_eq!(
            display_resource_name(&resources, None, Some(1), "fallback"),
            "fallback"
        );
    }

    #[test]
    fn shader_editor_update_preserves_unrelated_editors_and_rejects_ambiguity() {
        let existing = json!({"plugin":1,"editors":[{"id":"ResourceId::7","stage":4,"extra":true,"files":[{"name":"old","contents":"old"}]},{"id":"ResourceId::8","stage":0,"files":[]}]});
        let edit = json!({"id":"ResourceId::7","stage":4,"entryPoint":"main","encoding":5,"commandLine":"","tool":"Builtin","flags":{"@cmdline":""},"files":[{"name":"main.hlsl","contents":"中文"}]});
        let updated = upsert_shader_editor(&existing, &edit).unwrap();
        assert_eq!(updated["plugin"], 1);
        assert_eq!(updated["editors"][0]["extra"], true);
        assert_eq!(updated["editors"][0]["files"][0]["contents"], "中文");
        assert_eq!(updated["editors"][1], existing["editors"][1]);
        assert_eq!(
            shader_editor(&updated, "ResourceId::7", 4)
                .unwrap()
                .unwrap()["entryPoint"],
            "main"
        );
        let duplicate =
            json!({"editors":[{"id":"ResourceId::7","stage":4},{"id":"ResourceId::7","stage":4}]});
        assert!(upsert_shader_editor(&duplicate, &edit).is_err());
        assert!(upsert_shader_editor(&json!({"editors":"bad"}), &edit).is_err());
    }
}
