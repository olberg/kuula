//! Drive `serve` over in-memory pipes: one request per line in, one
//! response per line out, on `examples/hello` and on carts written into
//! a scratch root.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch root holding carts written by a test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kuula-mcp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }

    fn cart(&self, name: &str, main_lua: &str) {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.lua"), main_lua).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn call(id: u64, tool: &str, args: Value) -> String {
    request(id, "tools/call", json!({"name": tool, "arguments": args}))
}

/// Run a whole conversation and return the responses in order.
fn talk(root: PathBuf, lines: &[String]) -> Vec<Value> {
    talk_with(root, lines, None)
}

/// `talk` with a transport factory, so `run` may host or join.
fn talk_with(
    root: PathBuf,
    lines: &[String],
    transports: Option<kuula_mcp::TransportFactory>,
) -> Vec<Value> {
    let mut input = lines.join("\n");
    input.push('\n');
    let mut out = Vec::new();
    kuula_mcp::serve(Cursor::new(input.into_bytes()), &mut out, root, transports).unwrap();
    let text = String::from_utf8(out).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect()
}

fn init() -> String {
    request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    )
}

fn structured(v: &Value) -> &Value {
    &v["result"]["structuredContent"]
}

fn error_code(v: &Value) -> &str {
    assert_eq!(v["result"]["isError"], true, "{v}");
    v["result"]["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap()
}

#[test]
fn initialize_lists_tools_and_resources() {
    let replies = talk(
        examples(),
        &[
            init(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            request(2, "ping", json!({})),
            request(3, "tools/list", json!({})),
            request(4, "resources/list", json!({})),
            request(5, "resources/read", json!({"uri": "kuula://docs/api.md"})),
            request(6, "nope/method", json!({})),
            "this is not json".to_string(),
        ],
    );
    assert_eq!(replies.len(), 7, "the notification gets no reply");
    let r = &replies[0]["result"];
    assert_eq!(r["protocolVersion"], "2025-03-26", "client version echoed");
    assert_eq!(r["serverInfo"]["name"], "kuula-mcp");
    assert!(r["capabilities"]["tools"].is_object());
    assert!(r["capabilities"]["resources"].is_object());
    assert_eq!(replies[1]["result"], json!({}));
    let tools = replies[2]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "validate",
            "run",
            "replay",
            "step",
            "input",
            "screenshot",
            "state",
            "logs",
            "profile",
            "stop"
        ]
    );
    let res = replies[3]["result"]["resources"].as_array().unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0]["uri"], "kuula://docs/api.md");
    let text = replies[4]["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("cls"), "api.md documents cls");
    assert_eq!(replies[5]["error"]["code"], -32601);
    assert_eq!(replies[6]["error"]["code"], -32700);
    assert_eq!(replies[6]["id"], Value::Null);
}

#[test]
fn hello_runs_steps_screenshots_and_stops() {
    let replies = talk(
        examples(),
        &[
            init(),
            call(2, "validate", json!({"cart": "hello"})),
            call(3, "run", json!({"cart": "hello"})),
            call(
                4,
                "step",
                json!({"console": "c1", "frames": 3, "expect_frame": 1,
                       "input": [{"frames": 2, "buttons": ["right"]}, {"buttons": ["a"]}]}),
            ),
            call(5, "input", json!({"console": "c1", "frames": [16, 16, 0]})),
            call(6, "step", json!({"console": "c1", "frames": 2})),
            call(7, "screenshot", json!({"console": "c1"})),
            call(8, "state", json!({"console": "c1", "names": ["x"]})),
            call(9, "logs", json!({"console": "c1"})),
            call(10, "profile", json!({"console": "c1", "last": 2})),
            call(11, "stop", json!({"console": "c1"})),
            call(12, "step", json!({"console": "c1", "frames": 1})),
        ],
    );
    let v = structured(&replies[1]);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"], json!([]));

    let v = structured(&replies[2]);
    assert_eq!(v["console"], "c1");
    assert_eq!(v["frame"], 1);
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(320), Some(240))
    );
    assert_eq!(v["title"], "Hello");
    assert_eq!(v["running"], true);

    let v = structured(&replies[3]);
    assert_eq!(v["frame"], 4);
    assert_eq!(v["frames_run"], 3);
    assert_eq!(v["running"], true);
    assert!(v["fault"].is_null());

    let v = structured(&replies[4]);
    assert_eq!(v["queued"], 3);
    assert_eq!(v["frame"], 4);

    let v = structured(&replies[5]);
    assert_eq!(v["frame"], 6);

    let r = &replies[6]["result"];
    let image = &r["content"][0];
    assert_eq!(image["type"], "image");
    assert_eq!(image["mimeType"], "image/png");
    let data = image["data"].as_str().unwrap();
    assert!(data.starts_with("iVBORw0KGgo"), "PNG signature in base64");
    let v = structured(&replies[6]);
    assert_eq!(v["frame"], 6);
    assert_eq!(v["palette"].as_array().unwrap().len(), 128);
    assert_eq!(v["palette"][7], json!([255, 241, 232]));

    // `state` is answered either by the codec or by the
    // guest's refusal; both are data, never a protocol error.
    let s = &replies[7]["result"];
    assert!(s["structuredContent"].is_object(), "{s}");

    let v = structured(&replies[8]);
    assert_eq!(v["lines"], json!([]), "hello does not log");
    assert_eq!(v["truncated"], false);

    let v = structured(&replies[9]);
    let frames = v["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["frame"], 6);
    assert!(frames[1]["cycles"]["draw"].as_u64().unwrap() > 0);
    assert!(frames[1]["budget"].as_u64().unwrap() > 0);

    assert_eq!(structured(&replies[10])["stopped"], true);
    assert_eq!(error_code(&replies[11]), "stale_handle");
}

#[test]
fn a_hosting_run_reports_its_ticket_through_step() {
    use kuula_core::net::{MemoryTransport, Transport, MEMORY_TICKET};
    use std::cell::RefCell;
    use std::rc::Rc;
    let scratch = Scratch::new();
    scratch.cart(
        "host",
        "function _update() if not started then started = true net.host() end end",
    );
    std::fs::write(
        scratch.0.join("host").join("cart.toml"),
        "[cart]\nservices = [\"net\"]\n",
    )
    .unwrap();
    // The peer sides are kept so the pair stays open.
    let peers: Rc<RefCell<Vec<MemoryTransport>>> = Default::default();
    let stash = peers.clone();
    let transports: kuula_mcp::TransportFactory = Rc::new(move || {
        let (a, b) = MemoryTransport::pair();
        stash.borrow_mut().push(b);
        Box::new(a) as Box<dyn Transport>
    });
    let replies = talk_with(
        scratch.0.clone(),
        &[
            init(),
            call(
                2,
                "run",
                json!({"cart": "host", "frames": 1, "net": "host"}),
            ),
            call(3, "step", json!({"console": "c1", "frames": 1})),
            call(4, "step", json!({"console": "c1", "frames": 1})),
        ],
        Some(transports),
    );
    // Hosting is asynchronous: the run returns before the cart has even
    // asked (its first `_update` is frame 2), and the step in which it
    // asks reports hosting with no ticket yet.
    let run = structured(&replies[1]);
    assert_eq!(run["net"]["ticket"], Value::Null, "{run}");
    let step = structured(&replies[2]);
    assert_eq!(step["net"]["status"], "hosting", "{step}");
    assert_eq!(step["net"]["ticket"], Value::Null, "{step}");
    // The next step carries the ticket.
    let step = structured(&replies[3]);
    assert_eq!(step["net"]["status"], "hosting", "{step}");
    assert_eq!(step["net"]["ticket"], MEMORY_TICKET, "{step}");
}

#[test]
fn stale_frame_does_not_step() {
    let replies = talk(
        examples(),
        &[
            init(),
            call(2, "run", json!({"cart": "hello", "frames": 5})),
            call(
                3,
                "step",
                json!({"console": "c1", "frames": 4, "expect_frame": 4}),
            ),
            call(4, "step", json!({"console": "c1", "frames": 0})),
            call(5, "step", json!({"console": "c9", "frames": 1})),
            call(6, "screenshot", json!({"console": "c9"})),
        ],
    );
    assert_eq!(structured(&replies[1])["frame"], 5);
    assert_eq!(error_code(&replies[2]), "stale_frame");
    assert_eq!(structured(&replies[3])["frame"], 5, "nothing moved");
    assert_eq!(error_code(&replies[4]), "stale_handle");
    assert_eq!(error_code(&replies[5]), "stale_handle");
}

#[test]
fn the_console_cap_holds_and_stopping_frees_a_slot() {
    let mut lines = vec![init()];
    for i in 0..9 {
        lines.push(call(10 + i, "run", json!({"cart": "hello", "frames": 0})));
    }
    lines.push(call(30, "stop", json!({"console": "c3"})));
    lines.push(call(31, "run", json!({"cart": "hello", "frames": 0})));
    let replies = talk(examples(), &lines);
    for (i, r) in replies[1..9].iter().enumerate() {
        assert_eq!(structured(r)["console"], format!("c{}", i + 1));
    }
    assert_eq!(error_code(&replies[9]), "too_many_consoles");
    assert_eq!(structured(&replies[10])["stopped"], true);
    assert_eq!(
        structured(&replies[11])["console"],
        "c9",
        "a refused run mints no handle"
    );
}

#[test]
fn faults_are_data_and_paths_stay_in_the_root() {
    let scratch = Scratch::new();
    scratch.cart("boom", "function _update()\n  error('bang')\nend\n");
    scratch.cart("syntax", "x = = 1\n");
    scratch.cart(
        "loud",
        "function _draw()\n  print('hi\\x1b[31m', 'there')\nend\n",
    );
    let replies = talk(
        scratch.0.clone(),
        &[
            init(),
            call(2, "validate", json!({"cart": "syntax"})),
            call(3, "validate", json!({"cart": "boom"})),
            call(4, "run", json!({"cart": "boom", "frames": 10})),
            call(5, "step", json!({"console": "c1", "frames": 3})),
            call(6, "validate", json!({"cart": "../"})),
            call(7, "run", json!({"cart": "missing"})),
            call(8, "run", json!({"cart": "loud", "frames": 3})),
            call(9, "logs", json!({"console": "c2", "since_frame": 3})),
        ],
    );
    let v = structured(&replies[1]);
    assert_eq!(v["ok"], false);
    assert_eq!(v["errors"][0]["code"], "compile_error");
    assert_eq!(v["errors"][0]["file"], "main.lua");
    assert_eq!(v["errors"][0]["line"], 1);
    assert_eq!(v["errors"][0]["severity"], "error");

    let v = structured(&replies[2]);
    assert_eq!(v["ok"], true, "boom faults on frame 2, validate steps one");

    let v = structured(&replies[3]);
    assert_eq!(v["running"], false);
    assert_eq!(v["frame"], 2, "stops at the faulting frame");
    assert_eq!(v["fault"]["code"], "runtime_error");
    assert_eq!(v["fault"]["line"], 2);
    assert_eq!(v["fault"]["frame"], 2);
    assert_eq!(v["fault"]["message"], "bang");

    let v = structured(&replies[4]);
    assert_eq!(v["frames_run"], 0, "a faulted console does not step");
    assert_eq!(v["frame"], 2);

    assert_eq!(error_code(&replies[5]), "path_outside_root");
    assert_eq!(error_code(&replies[6]), "cart_not_found");

    let v = structured(&replies[7]);
    assert_eq!(v["console"], "c2");
    assert_eq!(
        v["log"][0]["text"], "hi[31m there",
        "escape removed, tab a space"
    );
    let v = structured(&replies[8]);
    assert_eq!(v["lines"].as_array().unwrap().len(), 1);
    assert_eq!(v["lines"][0]["frame"], 3);
}

#[test]
fn logs_are_bounded_and_results_are_capped() {
    let scratch = Scratch::new();
    scratch.cart(
        "chatty",
        "local line = string.rep('x', 1000)\nfunction _update()\n  for i = 1, 256 do print(line) end\nend\n",
    );
    let replies = talk(
        scratch.0.clone(),
        &[
            init(),
            call(2, "run", json!({"cart": "chatty", "frames": 10})),
            call(3, "logs", json!({"console": "c1"})),
            call(4, "logs", json!({"console": "c1", "since_frame": 10})),
        ],
    );
    // `run` returns every line it stepped: 9 frames of 256 lines is past
    // the 1 MiB cap, so the reply is a bounded error, not a flood.
    assert_eq!(error_code(&replies[1]), "result_too_large");
    let v = structured(&replies[2]);
    assert_eq!(v["truncated"], true);
    let lines = v["lines"].as_array().unwrap();
    assert!(lines.len() < 600 && lines.len() > 400, "{}", lines.len());
    assert!(
        lines[0]["frame"].as_u64().unwrap() >= 3,
        "only the last 2000 lines are kept, from frame 3 on: {}",
        lines[0]["frame"]
    );
    assert!(v["next_since_frame"].is_number());
    let v = structured(&replies[3]);
    assert_eq!(v["lines"].as_array().unwrap().len(), 256);
    assert_eq!(v["truncated"], false);
}

#[test]
fn bad_arguments_are_tool_errors_and_bad_calls_are_rpc_errors() {
    let replies = talk(
        examples(),
        &[
            init(),
            call(2, "run", json!({"cart": "hello", "frames": 1_000_000})),
            call(3, "run", json!({})),
            call(4, "deploy", json!({})),
            call(5, "run", json!({"cart": "hello", "frames": 0})),
            call(
                6,
                "step",
                json!({"console": "c1", "input": [{"buttons": ["start"]}]}),
            ),
            call(7, "input", json!({"console": "c1", "frames": [64]})),
            call(8, "profile", json!({"console": "c1", "last": 0})),
        ],
    );
    assert_eq!(error_code(&replies[1]), "invalid_arguments");
    assert_eq!(error_code(&replies[2]), "invalid_arguments");
    assert_eq!(replies[3]["error"]["code"], -32602);
    assert_eq!(error_code(&replies[5]), "invalid_input_script");
    assert_eq!(error_code(&replies[6]), "invalid_arguments");
    assert_eq!(error_code(&replies[7]), "invalid_arguments");
}

#[test]
fn an_over_long_line_is_refused_and_the_next_request_still_works() {
    let long = "{".repeat(kuula_mcp::MAX_LINE_BYTES + 1);
    let replies = talk(examples(), &[long, init()]);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32600, "{}", replies[0]);
    assert!(
        replies[1]["result"]["protocolVersion"].is_string(),
        "{}",
        replies[1]
    );
}

/// `run` with `record`, `stop` returning the transcript, `replay`
/// reproducing the screen, and the cart check.
#[test]
fn record_stop_and_replay_reproduce_the_screen() {
    let replies = talk(
        examples(),
        &[
            init(),
            call(
                2,
                "run",
                json!({"cart": "hello", "frames": 2, "record": true}),
            ),
            call(
                3,
                "step",
                json!({"console": "c1", "frames": 3,
                       "input": [{"frames": 2, "buttons": ["right"]}, {"buttons": ["a"]}]}),
            ),
            call(4, "screenshot", json!({"console": "c1"})),
            call(5, "stop", json!({"console": "c1"})),
            call(6, "run", json!({"cart": "hello", "frames": 1})),
            call(7, "stop", json!({"console": "c2"})),
        ],
    );
    assert_eq!(structured(&replies[1])["recording"], true);
    let png = replies[3]["result"]["content"][0]["data"]
        .as_str()
        .unwrap()
        .to_string();
    let v = structured(&replies[4]);
    assert_eq!(v["stopped"], true);
    assert_eq!(v["transcript_frames"], 5);
    assert_eq!(v["transcript_complete"], true);
    let text = v["transcript"].as_str().unwrap().to_string();
    assert!(text.starts_with("kuula-transcript 1\n"), "{text}");
    assert!(text.contains("{ buttons = 8, frames = 2 }"), "{text}");
    // A console that was not recording returns no transcript.
    assert!(structured(&replies[6])["transcript"].is_null());

    let replies = talk(
        examples(),
        &[
            init(),
            call(2, "replay", json!({"cart": "hello", "transcript": text})),
            call(3, "screenshot", json!({"console": "c1"})),
            call(4, "replay", json!({"cart": "saves", "transcript": text})),
            call(
                5,
                "replay",
                json!({"cart": "saves", "transcript": text, "any_cart": true}),
            ),
            call(
                6,
                "replay",
                json!({"cart": "hello", "transcript": "kuula-transcript 1\n"}),
            ),
            call(
                7,
                "replay",
                json!({"cart": "hello", "transcript": text, "frames": 2}),
            ),
        ],
    );
    let v = structured(&replies[1]);
    assert_eq!(v["console"], "c1");
    assert_eq!(v["frame"], 5);
    assert_eq!(v["frames_run"], 5);
    assert_eq!(v["transcript_frames"], 5);
    assert_eq!(v["queued"], 0);
    assert_eq!(v["verifies"], true);
    assert_eq!(
        replies[2]["result"]["content"][0]["data"].as_str().unwrap(),
        png,
        "the replay drew the recorded screen"
    );
    assert_eq!(error_code(&replies[3]), "transcript_cart_mismatch");
    let v = structured(&replies[4]);
    assert_eq!(v["verifies"], false);
    assert_eq!(v["frame"], 5);
    assert_eq!(error_code(&replies[5]), "invalid_transcript");
    let v = structured(&replies[6]);
    assert_eq!(v["frame"], 2);
    assert_eq!(v["queued"], 3, "the rest of the transcript waits for step");
}
