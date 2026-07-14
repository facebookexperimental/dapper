// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_session::ResponseContext;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct ControlPlaneResult<T> {
    pub result: T,
    pub context: Option<ResponseContext>,
}

impl<T: Serialize> ControlPlaneResult<T> {
    pub fn to_json_fields(&self) -> anyhow::Result<(String, String)> {
        let result_json = serde_json::to_string(&self.result)?;
        let context_json = match &self.context {
            Some(ctx) => serde_json::to_string(ctx)?,
            None => String::new(),
        };
        Ok((result_json, context_json))
    }
}

impl<T: DeserializeOwned> ControlPlaneResult<T> {
    pub fn from_proto_fields(result_json: String, context_json: String) -> anyhow::Result<Self> {
        if result_json.is_empty() {
            return Err(anyhow::anyhow!(
                "Server returned empty result_json; \
                 the server may be running an older version that does not support structured responses"
            ));
        }

        let result: T = serde_json::from_str(&result_json)?;
        let context = if context_json.is_empty() {
            None
        } else {
            Some(serde_json::from_str(&context_json)?)
        };

        Ok(Self { result, context })
    }
}

#[cfg(test)]
impl<T> ControlPlaneResult<T> {
    pub fn into_parts(self) -> (T, Option<ResponseContext>) {
        (self.result, self.context)
    }
}

#[cfg(test)]
mod tests {
    use dapper_session::ThreadsResult;

    use super::*;

    #[test]
    fn to_json_fields_structured() {
        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: None,
        };
        let (result_json, context_json) = result.to_json_fields().expect("serialize");
        assert!(!result_json.is_empty());
        assert!(context_json.is_empty());
    }

    #[test]
    fn to_json_fields_structured_with_context() {
        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: Some(ResponseContext::default()),
        };
        let (result_json, context_json) = result.to_json_fields().expect("serialize");
        assert!(!result_json.is_empty());
        assert!(!context_json.is_empty());
    }

    #[test]
    fn from_proto_fields_structured() {
        let original = ThreadsResult::default();
        let result_json = serde_json::to_string(&original).expect("serialize");

        let cp_result: ControlPlaneResult<ThreadsResult> =
            ControlPlaneResult::from_proto_fields(result_json, String::new())
                .expect("from_proto_fields");

        assert_eq!(cp_result.result, original);
        assert!(cp_result.context.is_none());
    }

    #[test]
    fn from_proto_fields_errors_on_empty_result_json() {
        let result =
            ControlPlaneResult::<ThreadsResult>::from_proto_fields(String::new(), String::new());
        assert!(result.is_err());
    }

    #[test]
    fn round_trip_through_json_fields() {
        let original_result = ThreadsResult::default();
        let original_context = ResponseContext::default();
        let cp_result = ControlPlaneResult {
            result: original_result.clone(),
            context: Some(original_context.clone()),
        };

        let (result_json, context_json) = cp_result.to_json_fields().expect("serialize");
        let restored: ControlPlaneResult<ThreadsResult> =
            ControlPlaneResult::from_proto_fields(result_json, context_json).expect("deserialize");

        assert_eq!(restored.result, original_result);
        assert_eq!(restored.context, Some(original_context));
    }
}
