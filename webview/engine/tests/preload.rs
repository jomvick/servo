//! Ce qu'un `UserScript` atteint vraiment, et ce que le frame top voit des autres frames.
//!
//! Un seul `#[test]` : `Opts` est un singleton de processus (voir `tests/tabs.rs`).
//!
//! D9 dit que l'injection doit être double parce que `UserScript` et le parcours
//! `contentWindow` depuis le top couvrent des frames disjoints. Cette mesure porte sur les
//! trois formes d'iframe que le walker rencontrera — `srcdoc`, `data:`, et une origine http
//! distincte — et dit laquelle tombe dans quel mécanisme.
//!
//! Un iframe cross-origin est précisément celui que le top ne peut pas lire : sa réponse ne
//! peut pas venir du top. L'enfant se signale donc lui-même, par une requête vers son propre
//! serveur — c'est ce qui distingue « le frame n'a pas chargé » de « le frame a chargé mais
//! l'injection ne l'atteint pas », et c'est ce qui valide l'agrégation par `postMessage` (D2).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use webview_core::{ErrorCode, ProfileId, TabId};
use webview_engine::{Engine, EngineConfig, EngineEvent};

const VIEWPORT: (u32, u32) = (800, 600);
/// Voir `tests/tabs.rs` : plafond du premier démarrage à froid, pas une mesure de latence.
const DEADLINE: Duration = Duration::from_secs(180);

/// Le preload minimal, mais dans la forme réelle du walker (D2) : il se signale, et il fait
/// connaître le frame au top par `postMessage` — seul canal légal d'un frame cross-origin.
/// Le top tient la liste des frames qui se sont annoncés, c'est ce que la mesure veut lire.
const PRELOAD: &str = r#"window.__egoPreload = true;
window.__egoBoxes = [];
window.addEventListener('message', function (event) {
    if (event.data && event.data.type === 'ego-hello') { window.__egoBoxes.push(event.data.href); }
});
if (window !== window.top) {
    try { window.top.postMessage({type: 'ego-hello', href: String(location.href).slice(0, 20)}, '*'); } catch (e) {}
}"#;

/// Un serveur http jetable, qui note chaque ligne de requête reçue : c'est le seul moyen
/// d'avoir une origine distincte du document top, ce qu'aucune data URL ne peut faire.
fn serve(pages: Vec<(&'static str, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("port libre");
    let port = listener.local_addr().expect("adresse locale").port();
    let journal: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = journal.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(clone) => clone,
                Err(_) => continue,
            });
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_owned();
            recorded.lock().expect("journal").push(path.clone());
            let body = pages
                .iter()
                .find(|(route, _)| *route == path)
                .map(|(_, body)| body.clone())
                .unwrap_or_default();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), journal)
}

/// L'enfant cross-origin rend public ce qu'il voit de lui-même. Une image plutôt qu'un
/// `fetch`, parce qu'une image ne dépend pas de la politique CSP du document.
const CHILD: &str = "<html><head><title>foreign</title></head><body><h1>foreign</h1>\
    <script>new Image().src = '/report?userscript=' + (window.__egoPreload === true);\
    </script></body></html>";

/// Pour chaque iframe : le preload y est-il entré, et le top y lit-il un document ? Les deux
/// réponses sont indépendantes, et c'est tout l'objet de la mesure.
const FRAMES: &str = r#"JSON.stringify(Array.prototype.map.call(document.querySelectorAll('iframe'), function (frame, index) {
    var seen = {index: index};
    try { seen.preload = frame.contentWindow.__egoPreload === true; } catch (e) { seen.preload = 'leve: ' + String(e).slice(0, 30); }
    try { seen.title = frame.contentDocument ? frame.contentDocument.title : 'pas de document'; } catch (e) { seen.title = 'leve: ' + String(e).slice(0, 30); }
    return seen;
}))"#;

fn field(frames: &[Value], index: usize, key: &str) -> String {
    frames
        .get(index)
        .and_then(|frame| frame.get(key))
        .map(|value| match value {
            Value::Bool(flag) => flag.to_string(),
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// Le journal de l'origine enfant, dès qu'une ligne `/report` y est entrée.
fn wait_report(journal: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    let started = Instant::now();
    loop {
        let seen = journal.lock().expect("journal").clone();
        if seen.iter().any(|path| path.starts_with("/report")) {
            return seen;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "l'enfant cross-origin n'a rien signalé ; journal : {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Seen {
    events: Vec<EngineEvent>,
}

impl Seen {
    fn pump(&mut self, engine: &mut Engine) {
        engine.pump();
        self.events.extend(engine.take_events());
    }

    fn wait_title(&mut self, engine: &mut Engine, wanted: &str) {
        let started = Instant::now();
        loop {
            self.pump(engine);
            let last = self.events.iter().rev().find_map(|event| match event {
                EngineEvent::TitleChanged { title, .. } => Some(title.as_str()),
                _ => None,
            });
            if last == Some(wanted) {
                return;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "le titre `{wanted}` n'est jamais arrivé ; dernier vu : {last:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Les enfants chargent après le top et ne poussent aucun événement ici : on attend que les
    /// trois iframes aient répondu — titre lu ou document refusé — ou que le délai soit passé.
    fn wait_children(&mut self, engine: &mut Engine, tab: TabId) -> String {
        let started = Instant::now();
        loop {
            self.pump(engine);
            let raw = engine
                .evaluate(tab, FRAMES)
                .unwrap_or_else(|_| Value::String("[]".into()));
            let text = raw.as_str().unwrap_or("[]").to_owned();
            let parsed: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
            let ready = parsed.iter().all(|frame| {
                frame
                    .get("title")
                    .and_then(Value::as_str)
                    .is_some_and(|title| !title.is_empty())
            });
            if ready || started.elapsed() > Duration::from_secs(10) {
                return text;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn preload_and_top_level_frame_traversal() {
    // Le document top est servi en http, une origine ; l'iframe enfant en prend une autre.
    let (foreign, journal) = serve(vec![("/child", CHILD.to_owned())]);
    let top_body = format!(
        "<html><head><title>top</title><style>body{{margin:0;background:#f00}}</style></head>\
         <body><iframe srcdoc=\"<title>src</title><h1>srcdoc</h1>\"></iframe>\
         <iframe src=\"data:text/html,<title>child-data</title><h1>d</h1>\"></iframe>\
         <iframe src=\"{foreign}/child\"></iframe></body></html>"
    );
    let (origin, _) = serve(vec![("/top", top_body)]);

    let mut engine = Engine::new(EngineConfig { viewport: VIEWPORT }).expect("moteur");
    engine.install_preload(PRELOAD);
    let space = engine.create_space("preload", ProfileId("default".into()));
    let tab = engine
        .create_tab(space, Some(&format!("{origin}/top")))
        .expect("onglet");

    let mut seen = Seen { events: Vec::new() };
    seen.wait_title(&mut engine, "top");

    // 1. Le preload est dans le frame top, et `evaluate` le lit.
    let top = engine
        .evaluate(tab, "String(window.__egoPreload)")
        .expect("top");
    assert_eq!(
        top,
        Value::String("true".into()),
        "le preload n'atteint pas le frame top"
    );

    // 2. Les trois enfants, vus depuis le top.
    let text = seen.wait_children(&mut engine, tab);
    let frames: Vec<Value> =
        serde_json::from_str(&text).expect("le comptage des iframes est du JSON");
    assert_eq!(
        frames.len(),
        3,
        "les trois iframes ne sont pas dans le document"
    );
    println!("MESURE-PRELOAD depuis_le_top={text}");

    // Le `srcdoc` se lit depuis le top et le preload n'y entre pas : c'est la frame du parcours.
    assert_eq!(field(&frames, 0, "title"), "src");
    assert_eq!(field(&frames, 0, "preload"), "false");
    // Les deux autres sont cross-origin : le top ne doit **pas** pouvoir les lire.
    for index in 1..3 {
        let title = field(&frames, index, "title");
        assert!(
            title.starts_with("leve:") || title == "pas de document",
            "iframe {index} : le top lit `{title}` dans un document cross-origin"
        );
    }

    // 3. L'agrégation du walker (D2) : un frame cross-origin peut-il joindre le top ?
    // `postMessage` est son seul canal légal, et tout le snapshot multi-frames en dépend.
    let started = Instant::now();
    let hellos: Vec<String> = loop {
        let raw = engine
            .evaluate(tab, "JSON.stringify(window.__egoBoxes)")
            .expect("les boîtes du top");
        let parsed: Vec<String> =
            serde_json::from_str(raw.as_str().unwrap_or("[]")).unwrap_or_default();
        if parsed.len() == 2 || started.elapsed() > Duration::from_secs(10) {
            break parsed;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    println!("MESURE-PRELOAD hellos={hellos:?}");
    assert!(
        hellos.iter().any(|href| href.starts_with("data:text/html")),
        "le frame `data:` n'a jamais joint le top : {hellos:?}"
    );
    assert!(
        hellos
            .iter()
            .any(|href| href.starts_with("http://127.0.0.1")),
        "le frame cross-origin n'a jamais joint le top : {hellos:?}"
    );

    // 4. Ce que l'enfant cross-origin dit de lui-même : il a bien chargé, et voici si l'injection
    // l'atteint. C'est la réponse qui décide de l'agrégation `postMessage` du walker (D2, D9).
    let journal = wait_report(&journal);
    println!("MESURE-PRELOAD journal_enfant={journal:?}");
    assert!(
        journal.iter().any(|path| path == "/child"),
        "l'iframe cross-origin n'a pas chargé : {journal:?}"
    );
    let reached = journal.iter().any(|path| path == "/report?userscript=true");
    println!("MESURE-PRELOAD injection_cross_origin={reached}");
    assert!(
        reached,
        "D9 ne tient pas dans le chemin embarqué : l'enfant cross-origin a chargé sans preload, \
         donc aucune injection ne l'atteint et le snapshot d'une iframe cross-origin est impossible"
    );

    // 4. Les deux erreurs que le pont doit traduire : script fautif, onglet inexistant.
    let broken = engine.evaluate(tab, "throw new Error('x')").unwrap_err();
    assert!(
        broken.message.contains('x'),
        "le message du script est perdu : {broken:?}"
    );
    assert_eq!(
        engine.evaluate(TabId(999), "1").unwrap_err().code,
        ErrorCode::WebContentsUnavailable
    );
}
