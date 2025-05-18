use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tokio_util::codec::Decoder;

#[derive(Default)]
pub struct JsonRpcFrameCodec;
impl Decoder for JsonRpcFrameCodec {
    type Item = tokio_util::bytes::Bytes;
    type Error = tokio::io::Error;
    fn decode(
        &mut self,
        src: &mut tokio_util::bytes::BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(end) = src
            .iter()
            .enumerate()
            .find_map(|(idx, &b)| (b == b'\n').then_some(idx))
        {
            let line = src.split_to(end);
            let _char_next_line = src.split_to(1);
            Ok(Some(line.freeze()))
        } else {
            Ok(None)
        }
    }
}

/// Combines multiple JSON schemas into one large schema
pub struct SchemaCombiner {
    schemas: HashMap<String, Value>,
    descriptions: HashMap<String, String>,
}

impl SchemaCombiner {
    /// Create a new SchemaCombiner
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
            descriptions: HashMap::new(),
        }
    }

    /// Add a JSON schema from a file with description
    #[allow(dead_code)]
    pub fn add_schema_from_file<P: AsRef<Path>>(
        &mut self,
        name: &str,
        path: P,
        description: Option<String>,
    ) -> Result<()> {
        let content = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read schema file: {:?}", path.as_ref()))?;

        let schema: Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse schema JSON: {:?}", path.as_ref()))?;

        // Save schema with $schema keyword removed
        let cleaned_schema = self.clean_schema(schema);
        self.schemas.insert(name.to_string(), cleaned_schema);

        // Save description if provided
        if let Some(desc) = description {
            self.descriptions.insert(name.to_string(), desc);
        }

        Ok(())
    }

    /// Add a JSON schema from a string with description
    pub fn add_schema_from_string(
        &mut self,
        name: &str,
        schema_str: &str,
        description: Option<String>,
    ) -> Result<()> {
        let schema: Value = serde_json::from_str(schema_str)
            .with_context(|| "Failed to parse schema JSON from string")?;

        // Save schema with $schema keyword removed
        let cleaned_schema = self.clean_schema(schema);
        self.schemas.insert(name.to_string(), cleaned_schema);

        // Save description if provided
        if let Some(desc) = description {
            self.descriptions.insert(name.to_string(), desc);
        }

        Ok(())
    }

    /// Add description for an existing schema
    #[allow(dead_code)]
    pub fn add_description(&mut self, name: &str, description: String) -> Result<()> {
        if !self.schemas.contains_key(name) {
            return Err(anyhow::anyhow!(
                "Schema with name '{}' does not exist",
                name
            ));
        }

        self.descriptions.insert(name.to_string(), description);
        Ok(())
    }

    /// Remove $schema keyword from a schema
    fn clean_schema(&self, schema: Value) -> Value {
        if let Value::Object(mut obj) = schema {
            // Remove $schema keyword
            obj.remove("$schema");

            // Recursively remove $schema from sub-schemas
            let cleaned_obj = Self::clean_object(obj);
            Value::Object(cleaned_obj)
        } else {
            schema
        }
    }

    /// Recursively remove $schema keyword from all sub-schemas in an object
    fn clean_object(obj: Map<String, Value>) -> Map<String, Value> {
        let mut result = Map::new();

        for (key, value) in obj {
            let cleaned_value = match value {
                Value::Object(sub_obj) => {
                    let mut new_obj = sub_obj.clone();
                    new_obj.remove("$schema");
                    Value::Object(Self::clean_object(new_obj))
                }
                Value::Array(arr) => Value::Array(
                    arr.into_iter()
                        .map(|item| match item {
                            Value::Object(sub_obj) => Value::Object(Self::clean_object(sub_obj)),
                            _ => item,
                        })
                        .collect(),
                ),
                _ => value,
            };

            result.insert(key, cleaned_value);
        }

        result
    }

    /// Generate the combined JSON schema with descriptions
    pub fn generate_combined_schema(&self) -> Result<serde_json::Map<String, Value>> {
        // Create base schema
        let mut combined = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {},
            "required": []
        });

        let mut properties_map = Map::new();
        let mut required_vec = Vec::new();

        for (name, schema) in &self.schemas {
            // XXX clone
            let mut schema_obj = schema.clone();

            if let Some(desc) = self.descriptions.get(name) {
                if let Value::Object(ref mut obj) = schema_obj {
                    obj.insert("description".to_string(), Value::String(desc.clone()));
                }
            }

            properties_map.insert(name.clone(), schema_obj);
            required_vec.push(json!(name));
        }

        if let Value::Object(ref mut obj) = combined {
            obj.insert("properties".to_string(), Value::Object(properties_map));
            obj.insert("required".to_string(), Value::Array(required_vec));
        }

        combined
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Failed to get combined schema object"))
    }

    /// Save the combined schema to a file
    #[allow(dead_code)]
    pub fn save_combined_schema<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let combined = self.generate_combined_schema()?;
        let json_str = serde_json::to_string_pretty(&combined)
            .context("Failed to serialize combined schema")?;

        fs::write(&path, json_str)
            .with_context(|| format!("Failed to write combined schema to {:?}", path.as_ref()))?;

        Ok(())
    }
}
