use serde_json::Value;

use crate::osc::VRCHAT_CLIENT_SERVICE;

use vrchat_osc::{
    Error, VRChatOSC,
    models::{OscNode, OscType as OscQueryType, OscValue},
    rosc::{OscMessage, OscPacket, OscType},
};

pub async fn get_all(vrchat_osc: &VRChatOSC) -> Result<Option<OscNode>, Error> {
    Ok(vrchat_osc
        .get_parameter("/", VRCHAT_CLIENT_SERVICE)
        .await?
        .into_iter()
        .next()
        .map(|(_, node)| node))
}

pub async fn get(vrchat_osc: &VRChatOSC, parameter: &str) -> Result<Option<OscNode>, Error> {
    Ok(vrchat_osc
        .get_parameter(parameter, VRCHAT_CLIENT_SERVICE)
        .await?
        .into_iter()
        .next()
        .map(|(_, node)| node))
}

pub fn set_value(node: &mut OscNode, parameter: &str, value: Vec<OscValue>) -> bool {
    let Some(node) = parameter
        .trim_matches('/')
        .split('/')
        .try_fold(node, |node, segment| node.contents.get_mut(segment))
    else {
        return false;
    };

    node.value = Some(value);
    true
}

pub fn value(node: OscNode) -> Result<Option<Value>, serde_json::Error> {
    let Some(values) = node.value else {
        return Ok(None);
    };

    match values.as_slice() {
        [value] => serde_json::to_value(value),
        _ => serde_json::to_value(values),
    }
    .map(Some)
}

pub async fn send(vrchat_osc: &VRChatOSC, parameter: &str, value: Value) -> Result<(), SendError> {
    let node = get(vrchat_osc, parameter)
        .await
        .map_err(SendError::Query)?
        .ok_or(SendError::NotFound)?;
    let argument = encode_value(&node, value)?;

    vrchat_osc
        .send(
            OscPacket::Message(OscMessage {
                addr: parameter.to_owned(),
                args: vec![argument],
            }),
            VRCHAT_CLIENT_SERVICE,
        )
        .await
        .map_err(SendError::Send)
}

fn encode_value(node: &OscNode, value: Value) -> Result<OscType, SendError> {
    let Some(value_type) = node.get_single_osc_type() else {
        return Err(SendError::UnsupportedType);
    };

    match value_type {
        OscQueryType::Int32 => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(OscType::Int)
            .ok_or(SendError::InvalidValue("a 32-bit integer")),
        OscQueryType::Float32 => value
            .as_f64()
            .filter(|value| value.is_finite())
            .and_then(|value| {
                let value = value as f32;
                value.is_finite().then_some(value)
            })
            .map(OscType::Float)
            .ok_or(SendError::InvalidValue("a finite number")),
        OscQueryType::OscString | OscQueryType::Symbol => value
            .as_str()
            .map(|value| OscType::String(value.to_owned()))
            .ok_or(SendError::InvalidValue("a string")),
        OscQueryType::True | OscQueryType::False => value
            .as_bool()
            .map(OscType::Bool)
            .ok_or(SendError::InvalidValue("a boolean")),
        _ => Err(SendError::UnsupportedType),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("unable to query parameter: {0}")]
    Query(Error),
    #[error("unable to send parameter: {0}")]
    Send(Error),
    #[error("parameter was not found")]
    NotFound,
    #[error("parameter type is not supported")]
    UnsupportedType,
    #[error("request body must be {0}")]
    InvalidValue(&'static str),
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vrchat_osc::{
        models::{OscNode, OscType as OscQueryType, OscTypeTag, OscValue},
        rosc::OscType,
    };

    use super::{SendError, encode_value, set_value, value};

    fn node(value_type: OscQueryType) -> OscNode {
        OscNode {
            r#type: Some(OscTypeTag::new(vec![value_type])),
            ..Default::default()
        }
    }

    #[test]
    fn encodes_values_using_the_parameter_type() {
        assert!(matches!(
            encode_value(&node(OscQueryType::Float32), json!(1)),
            Ok(OscType::Float(value)) if value == 1.0
        ));
        assert!(matches!(
            encode_value(&node(OscQueryType::True), json!(true)),
            Ok(OscType::Bool(true))
        ));
    }

    #[test]
    fn rejects_a_value_that_does_not_match_the_parameter_type() {
        assert!(matches!(
            encode_value(&node(OscQueryType::Int32), json!(1.5)),
            Err(SendError::InvalidValue("a 32-bit integer"))
        ));
    }

    #[test]
    fn replaces_a_nested_parameter_value() {
        let mut root = OscNode {
            contents: [(
                "usercamera".to_owned(),
                OscNode {
                    contents: [("Pose".to_owned(), OscNode::default())].into(),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };

        assert!(set_value(
            &mut root,
            "/usercamera/Pose",
            vec![vrchat_osc::models::OscValue::Float(1.0)],
        ));
        assert!(matches!(
            root.contents["usercamera"].contents["Pose"].value.as_deref(),
            Some([vrchat_osc::models::OscValue::Float(value)]) if *value == 1.0
        ));
    }

    #[test]
    fn serializes_single_values_directly_and_multiple_values_as_an_array() {
        let single = OscNode {
            value: Some(vec![OscValue::Int(1)]),
            ..Default::default()
        };
        let multiple = OscNode {
            value: Some(vec![OscValue::Int(1), OscValue::Bool(true)]),
            ..Default::default()
        };

        assert_eq!(value(single).unwrap(), Some(json!(1)));
        assert_eq!(value(multiple).unwrap(), Some(json!([1, true])));
    }
}
