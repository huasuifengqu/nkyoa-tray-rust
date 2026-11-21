# 第三方组件与许可

本产品包含以下第三方开源组件。`Cargo.lock` 是依赖版本的权威清单；
分发时应同时附上本文件与 `licenses/` 目录中的许可文本。

## 界面

- **Slint 1.19.0** — 许可：GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0
  OR LicenseRef-Slint-Software-3.0。本产品按 **Slint Royalty-free 2.0** 使用；
  许可文本见 `licenses/Slint-Royalty-free-2.0.md`。

## Windows 平台

- **windows-rs（windows / windows-registry / windows-result / windows-strings）**
  — MIT OR Apache-2.0。
- **native-tls / schannel** — MIT OR Apache-2.0。

## 基础设施

- **tokio / tokio-util** — MIT。
- **reqwest / hyper / h2 / tower / tower-http** — MIT OR Apache-2.0。
- **serde / serde_json** — MIT OR Apache-2.0。
- **url / idna** — MIT OR Apache-2.0。
- **uuid** — Apache-2.0 OR MIT。
- **time** — MIT OR Apache-2.0。
- **sha2 / digest / generic-array** — MIT OR Apache-2.0。
- **async-trait** — MIT OR Apache-2.0。
- **httpdate** — MIT OR Apache-2.0。
- **zeroize** — Apache-2.0 OR MIT。

`tempfile` 仅作为测试依赖，不进入发布产物。
