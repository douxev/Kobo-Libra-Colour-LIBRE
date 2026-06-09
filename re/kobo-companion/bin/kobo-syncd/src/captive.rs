// Connectivity-check / captive-portal responder — BIND 127.0.0.1 ONLY.
//
// Why: when the firewall (opds-netcut) cuts the internet, the firmware's mandatory
// connectivity check (http://captive.apple.com/hotspot-detect.html — see RECONSTRUCTION
// §5.2/§6) is dropped, so Nickel/tolino believes it is OFFLINE and refuses to open the
// web browser (which also makes the local config panel at 127.0.0.1:8080 unreachable).
//
// Fix: opds-netcut adds an iptables NAT rule that redirects outbound TCP :80 (to any
// non-LAN host) to this responder, which replies "Success" so the check passes WITHOUT
// a single byte leaving the device. No external contact ever happens — purely loopback.
//
// PORT must match CAPTIVE_PORT in re/netcut/opds-netcut.
use std::io::Cursor;
use tiny_http::{Header, Response, Server};

pub const PORT: u16 = 8081;

// Apple/Windows/generic captive checks look for the literal "Success" in a 200 body.
const SUCCESS_HTML: &str = "<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>\n";

pub fn serve() {
    let bind = format!("127.0.0.1:{}", PORT);
    let server = match Server::http(&bind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("captive: cannot listen on {} : {}", bind, e);
            return;
        }
    };
    eprintln!("captive: connectivity-check responder on http://{}/ (loopback only)", bind);
    for req in server.incoming_requests() {
        let path = req.url().splitn(2, '?').next().unwrap_or("/").to_string();
        let resp: Response<Cursor<Vec<u8>>> = if path.contains("generate_204") {
            // Android/Chrome-style probe expects HTTP 204 No Content (empty body).
            Response::from_string("").with_status_code(204u16)
        } else {
            // Apple/Windows/captive probes expect 200 + a body containing "Success".
            let ct = Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap();
            Response::from_string(SUCCESS_HTML).with_header(ct)
        };
        let _ = req.respond(resp);
    }
}
