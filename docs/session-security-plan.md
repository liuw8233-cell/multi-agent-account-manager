# 多平台登录态与浏览器隔离：安全实施说明

## 当前已确认

- 桌面端使用 Tauri 系统 WebView。macOS 使用 UUID 派生的 `data_store_identifier`，Windows 使用独立 `profiles/<uuid>` 数据目录。
- 这提供浏览器数据隔离，**不代表**已经实现 Canvas/WebGL/字体/时区/代理出口等完整指纹控制。
- `museAPI-video2` 使用 Chrome DevTools Protocol 的 `Storage.getCookies` 读取登录态；它操作的是 Chrome Profile，而非此项目的 Tauri WebView Profile，不能直接复制过来。
- Google 账号宜采用系统浏览器 + OAuth 2.0（Authorization Code + PKCE），而非读取 Google 网页 Cookie。

## 本分支改动

- 限制 `blob:` 顶层导航：仅允许 URL 内嵌的 HTTPS 来源与当前服务的已验证回调来源对应，避免任意 `blob:https://... ` 获得跳转许可。
- 加入基础单元测试，覆盖本服务 blob、跨服务 blob、外部域名、HTTP 和 Google OAuth 路由。

## 已实现：本地手动 Cookie 读取

桌面端在当前活跃账号登录后提供 `Cookie` 按钮，用户确认后通过 Tauri WebView `cookies_for_url` 读取当前服务入口的 Cookie（含 HttpOnly），复制到本机剪贴板；不自动上传、不写入 `accounts.json`、不打印日志。Android 未提供此功能。必须注意：剪贴板中的登录凭据可被其他本机应用读取，使用后应清理。

## Hark Google 登录的已知限制

Hark 在系统 WebView 的 Google 登录流程可能提示“此版本的 Hark 应用无法使用 Google 登录”。当前程序会拦截 Google OAuth WebView 导航，避免违反第三方安全策略。打开系统浏览器可以完成正常的 Google 授权，但 **不能自动把授权后的登录态回传到 Hark 独立 WebView**；只有 Hark 官方提供可用的原生客户端 OAuth 回调或其他受支持的授权方式，才能真正闭环。现阶段请使用 Hark 邮箱登录。不能通过修改 User-Agent 冒充 Chrome 来声称修复此限制。

## 后续实施的最小安全接口

`ProviderAdapter` 应仅提供：

1. `login_start(account_id)`：在该账号独立 Profile 中打开登录页。
2. `auth_status(account_id)`：返回 `unknown/authenticated/logged_out` 与最后验证时间，不返回凭据。
3. `account_identity(account_id)`：读取已授权的公开用户标识或平台 API 返回的昵称。
4. `quota(account_id)`：复用现有额度逻辑。
5. `logout(account_id)`：清理该账号会话及对应的加密凭据记录。

**Cookie 读取如果确有必要**，必须绑定当前账号的独立 WebView 数据存储、限定该平台的确切域名，由用户主动触发；不在 UI、日志、IPC 事件和 `accounts.json` 返回原始值。若要为 API 后端提供登录态，应在本机加密存储，并使用受控的本地接口按需调用，不实现任意远程 Cookie 导出功能。

## 平台扩展顺序

- Muse：在现有 Profile 隔离和认证状态的基础上，验证需要哪些官方或可授权接口。
- Hark：独立适配器，避免共享 Muse 的 Cookie 域和身份逻辑。
- Google：注册 OAuth 客户端，采用授权码 + PKCE、严格回调校验和最小范围 Scope；凭据保存在操作系统密钥存储中。
- 后续其他平台：只接入其明确支持的授权方式，不假设都能通过 Cookie 迁移。

## 验证

在可用的 Rust/Tauri 构建环境执行 `cd src-tauri && cargo test`；在项目根目录执行 `npm ci && npm run build`。还需要分别进行 Windows WebView2 和 macOS WKWebView 的真实多账号隔离测试。GitHub API 直接编辑不等同于执行了这些测试。
