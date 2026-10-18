//! 内置调试台静态页（由守护进程直接提供，无需设备上有 web 服务器）。
//!
//! 页面在编译期嵌入二进制（等价于其他实现里的 go:embed），
//! 所以安装后 `http://<路由器>:9527/` 立刻可用。
//!
//! 骨架阶段只托管首页、迷你页与三个主题样式表；其余资源按需补。

use tiny_http::{Header, Request, Response, StatusCode};

use crate::error::Result;

const INDEX: &str = include_str!("../../web/index.html");
const MINI: &str = include_str!("../../web/mini.html");
const CSS_OUTLINE: &str = include_str!("../../web/layouts/outline.css");
const CSS_PANEL: &str = include_str!("../../web/layouts/panel.css");
const CSS_SOFT: &str = include_str!("../../web/layouts/soft.css");
const CSS_STANDARD: &str = include_str!("../../web/layouts/standard.css");

/// 处理一个非 `/api/*` 的 GET 请求。
pub fn serve(req: Request, path: &str) -> Result<()> {
    let (body, content_type): (&str, &str) = match path {
        "/" | "/index.html" | "/ui" => (INDEX, "text/html; charset=utf-8"),
        "/mini" | "/mini.html" => (MINI, "text/html; charset=utf-8"),
        "/layouts/outline.css" => (CSS_OUTLINE, "text/css; charset=utf-8"),
        "/layouts/panel.css" => (CSS_PANEL, "text/css; charset=utf-8"),
        "/layouts/soft.css" => (CSS_SOFT, "text/css; charset=utf-8"),
        "/layouts/standard.css" => (CSS_STANDARD, "text/css; charset=utf-8"),
        _ => return not_found(req),
    };
    let header =
        Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("static header");
    let response = Response::from_string(body).with_header(header);
    req.respond(response)?;
    Ok(())
}

fn not_found(req: Request) -> Result<()> {
    let response = Response::from_string("404 Not Found").with_status_code(StatusCode(404));
    req.respond(response)?;
    Ok(())
}
