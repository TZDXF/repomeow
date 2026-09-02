//! TaggedError 组合器:对齐 `packages/agent/src/harness/result.ts`。
//!
//! TS 通过运行时工厂 `TaggedError(tag)` 生成带 `_tag` 的 Error 子类;Rust 无
//! 运行时类构造,选择 **thiserror 派生 + 声明宏** 方案:`tagged_error!` 宏为每个
//! 错误定义一个具名结构体(message + 载荷字段),派生 `Error`/`Clone`/`Serialize`,
//! 并实现 [`TaggedErrorValue`] 提供 `tag()`(对齐 `_tag`)与 `to_error_json()`
//! (对齐 `toJSON()` 的 `{_tag, message, ...payload}` 形状)。`is(value)` 静态
//! 判别由标准库的 `Error::downcast_ref` 取代。
//! `Result`/`isOk`/`isErr` 与 harness/types.rs 重复,统一收敛到 types.rs。

use serde::Serialize;

/// 带稳定判别标签的错误值(对齐 TS `TaggedErrorValue`)。
pub trait TaggedErrorValue: std::error::Error + Serialize {
    /// 错误判别标签(对齐 TS `_tag`)。
    fn tag(&self) -> &'static str;

    /// 对齐 TS `toJSON()`:`{_tag, message, ...payload}`。
    fn to_error_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_else(|_| serde_json::Value::Null);
        if let serde_json::Value::Object(map) = &mut value {
            map.insert(
                "_tag".to_string(),
                serde_json::Value::String(self.tag().to_string()),
            );
        }
        value
    }
}

/// 定义一个带标签的错误结构体(thiserror 派生 + TaggedErrorValue 实现)。
///
/// 首字段固定为 `message`;其余为错误载荷。载荷字段经 serde(camelCase)进入
/// `to_error_json`,与 TS `Object.assign(this, props)` + `toJSON` 输出兼容。
#[macro_export]
macro_rules! harness_tagged_error {
    ($(#[$meta:meta])* $name:ident, $tag:literal { }) => {
        #[derive(Debug, Clone, ::serde::Serialize, ::thiserror::Error)]
        #[serde(rename_all = "camelCase")]
        #[error("{message}")]
        $(#[$meta])*
        pub struct $name {
            pub message: String,
        }

        impl $name {
            pub fn new(message: impl ::std::convert::Into<String>) -> Self {
                Self { message: message.into() }
            }
        }

        impl $crate::agent::harness::result::TaggedErrorValue for $name {
            fn tag(&self) -> &'static str {
                $tag
            }
        }
    };
    ($(#[$meta:meta])* $name:ident, $tag:literal { $($(#[$fmeta:meta])* $fvis:vis $field:ident: $ty:ty),+ $(,)? }) => {
        #[derive(Debug, Clone, ::serde::Serialize, ::thiserror::Error)]
        #[serde(rename_all = "camelCase")]
        #[error("{message}")]
        $(#[$meta])*
        pub struct $name {
            pub message: String,
            $($(#[$fmeta])* $fvis $field: $ty,)+
        }

        impl $name {
            pub fn new(message: impl ::std::convert::Into<String>, $($field: $ty),+) -> Self {
                Self { message: message.into(), $($field,)+ }
            }
        }

        impl $crate::agent::harness::result::TaggedErrorValue for $name {
            fn tag(&self) -> &'static str {
                $tag
            }
        }
    };
}
