//! RFC3339 UTC 时间的序列化。
//!
//! 服务端与 `.NET` 客户端都要求时间为零偏移；本模块把 `+00:00` 统一写成 `Z`，
//! 保证 `state.json` 与 .NET 版可互相读取。

use serde::{Deserialize, Deserializer, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub fn serialize<S>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let text = value.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
    let text = match text.strip_suffix("+00:00") {
        Some(prefix) => format!("{prefix}Z"),
        None => text,
    };
    serializer.serialize_str(&text)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    let value = OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)?;
    if !value.offset().is_utc() {
        return Err(serde::de::Error::custom("时间必须为 UTC"));
    }
    // .NET 侧 DateTimeOffset 默认值（0001-01-01T00:00:00Z）同样视为无效。
    if OffsetDateTime::from_unix_timestamp(-62_135_596_800).ok() == Some(value) {
        return Err(serde::de::Error::custom("时间不能为默认值"));
    }
    Ok(value)
}
