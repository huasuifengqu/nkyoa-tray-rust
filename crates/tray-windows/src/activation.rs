//! 受控协议激活参数。
//!
//! 只接受 `nkyoa-notify-rust://open/<64 位小写十六进制配置标识>`；不携带
//! 任意 URL、命令或查询参数。

use crate::brand::PROTOCOL_PREFIX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub profile_id: Option<String>,
}

impl Activation {
    pub const SHOW_SETTINGS: Self = Self { profile_id: None };

    pub fn is_profile(value: &str) -> bool {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    pub fn from_uri(uri: &str) -> Option<Self> {
        let profile_id = uri.strip_prefix(PROTOCOL_PREFIX)?;
        Self::is_profile(profile_id).then(|| Self {
            profile_id: Some(profile_id.to_owned()),
        })
    }

    pub fn to_uri(profile_id: &str) -> Option<String> {
        Self::is_profile(profile_id).then(|| format!("{PROTOCOL_PREFIX}{profile_id}"))
    }

    pub fn to_message(&self) -> String {
        match &self.profile_id {
            None => "show".to_owned(),
            Some(profile_id) => format!("open:{profile_id}"),
        }
    }

    pub fn from_message(message: &str) -> Option<Self> {
        if message == "show" {
            return Some(Self::SHOW_SETTINGS);
        }
        let profile_id = message.strip_prefix("open:")?;
        Self::is_profile(profile_id).then(|| Self {
            profile_id: Some(profile_id.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> String {
        "a".repeat(64)
    }

    #[test]
    fn accepts_only_the_fixed_protocol_and_profile_shape() {
        let id = profile();
        let uri = format!("nkyoa-notify-rust://open/{id}");
        assert_eq!(
            Activation::from_uri(&uri),
            Some(Activation {
                profile_id: Some(id.clone())
            })
        );
        assert_eq!(Activation::to_uri(&id), Some(uri.clone()));
        assert_eq!(
            Activation::from_uri(&uri).unwrap().to_message(),
            format!("open:{id}")
        );
        assert_eq!(
            Activation::from_message(&format!("open:{id}")),
            Activation::from_uri(&uri)
        );
        assert_eq!(Activation::SHOW_SETTINGS.to_message(), "show");
        assert_eq!(
            Activation::from_message("show"),
            Some(Activation::SHOW_SETTINGS)
        );

        for bad in [
            "https://oa.test/".to_owned(),
            "nkyoa-notify-rust://open/".to_owned(),
            format!("nkyoa-notify-rust://open/{id}?url=https://oa.test"),
            format!("nkyoa-notify-rust://open/{id}/.."),
            "A".repeat(64),
            "g".repeat(64),
            id[..63].to_owned(),
        ] {
            assert!(Activation::from_uri(&bad).is_none(), "应拒绝 {bad}");
        }
        assert!(Activation::from_message("open:").is_none());
        assert!(Activation::from_message("show ").is_none());
    }
}
