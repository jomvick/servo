/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 0 probes for the `webview` shell: what the software (CPU) pixel exit
//! really costs, in which byte order it comes out, and how far a `UserScript`
//! propagates into subframes.
//!
//! These are measurements, not assertions of intended behaviour: they print
//! `PROBE-*` lines and only fail when the plumbing itself is broken.
//!
//! Run with:
//!   cargo test -p servo --test phase0_probe -- --nocapture --test-threads=1

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use euclid::default::Point2D;
use http::Response;
use http::header::CONTENT_TYPE;
use hyper::body::{Bytes, Incoming};
use net::test_util::{make_body, make_server};
use servo::{
    DeviceIntRect, DevicePoint, InputEvent, InputEventId, InputEventResult, LoadStatus,
    UserContentManager, UserScript, WebView, WebViewBuilder, WebViewDelegate, WheelDelta,
    WheelEvent, WheelMode,
};
use url::Url;

use crate::common::{ServoTest, WebViewDelegateImpl};

/// The test harness creates a 500x500 `SoftwareRenderingContext`.
const SIDE: i32 = 500;

/// Page de mesure : un carré rouge statif (référence de couleur pour l'ordre des
/// octets) sur un document assez haut pour défiler. Le défilement est ce qui rend
/// les frames sales : sans contenu qui bouge, `paint()` ne rend rien du tout et on
/// mesurerait une non-peinture. (Une `animation` CSS ne fait pas avancer l'image dans
/// ce harnais — vérifié, le pixel échantillonné ne change pas sur 250 ms.)
///
/// Les `#` de CSS sont écrits `%23` : dans une data URL, un `#` non échappé ouvre
/// le fragment et tronque la feuille de style.
///
/// 20000 px de haut : la mesure défile ~2400 px par taille et par série d'essais. Une page
/// plus courte finirait en bas, et les `paint()` suivants ne rendraient plus rien de sale
/// tout en étant chronométrés comme un vrai rendu.
const DATA_PAGE: &str = "data:text/html,\
    <style>body{margin:0;background:%23fff}\
    .s{width:200px;height:200px;background:%23f00}\
    .tall{height:20000px;background:repeating-linear-gradient(%23fff,%23fff 40px,%23000 40px,%23000 80px)}\
    </style><div class=s></div><div class=tall></div>";

fn html(content: &[u8]) -> Vec<u8> {
    let mut body = b"<!DOCTYPE html><meta charset=utf-8>".to_vec();
    body.extend_from_slice(content);
    body
}

type HtmlResponse = Response<http_body_util::combinators::BoxBody<Bytes, hyper::Error>>;

fn serve(
    content: Vec<u8>,
) -> impl Fn(http::Request<Incoming>, &mut HtmlResponse) + Send + Sync + 'static {
    move |_request: http::Request<Incoming>, response: &mut HtmlResponse| {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
        *response.body_mut() = make_body(content.clone());
    }
}

/// Document de test qui se signale lui-meme dans la console, independamment de
/// tout `UserScript` : c'est ce qui permet de distinguer « le frame n'a pas
/// charge » de « le frame est charge mais le script utilisateur ne l'atteint pas ».
fn page(tag: &str, body: &str) -> Vec<u8> {
    html(format!("<script>console.log('NATURAL|{tag}')</script>{body}").as_bytes())
}

/// Delegate de mesure. Contrairement a `WebViewDelegateImpl` du harnais, il **ne peint
/// pas** dans `notify_new_frame_ready` : s'il peignait, la frame sale serait consommee
/// avant la mesure et `paint()` chronometrerait une non-peinture. Compter les
/// notifications suffit a savoir quand la scene est reellement sale.
#[derive(Default)]
struct ProbeDelegate {
    frames_ready: Cell<usize>,
    events_handled: Cell<usize>,
}

impl WebViewDelegate for ProbeDelegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.frames_ready.set(self.frames_ready.get() + 1);
    }

    fn notify_input_event_handled(
        &self,
        _webview: WebView,
        _id: InputEventId,
        _result: InputEventResult,
    ) {
        self.events_handled.set(self.events_handled.get() + 1);
    }
}

fn spin_until(servo_test: &ServoTest, timeout: Duration, ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while !ready() {
        if Instant::now() >= deadline {
            return false;
        }
        servo_test.servo.spin_event_loop();
        thread::sleep(Duration::from_millis(1));
    }
    true
}

/// Attend qu'aucune nouvelle scene n'arrive pendant `quiet` : `LoadStatus::Complete` et le
/// premier `notify_new_frame_ready` ne garantissent pas que le contenu soit dans la scene
/// (verifie : le premier `paint()` d'un document a data: rend une page blanche, le carre
/// rouge n'apparait qu'a la scene suivante).
fn wait_for_last_scene(servo_test: &ServoTest, delegate: &Rc<ProbeDelegate>, quiet: Duration) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let count = delegate.frames_ready.get();
        servo_test.servo.spin_event_loop();
        thread::sleep(quiet);
        if delegate.frames_ready.get() == count {
            return;
        }
    }
}

#[test]
fn probe_frame_path_and_cost() {
    let servo_test = ServoTest::new();
    let delegate = Rc::<ProbeDelegate>::default();
    let webview = WebViewBuilder::new(servo_test.servo(), servo_test.rendering_context.clone())
        .url(Url::parse(DATA_PAGE).unwrap())
        .delegate(delegate.clone())
        .build();

    servo_test.spin(|| webview.load_status() != LoadStatus::Complete);
    spin_until(&servo_test, Duration::from_secs(5), || {
        delegate.frames_ready.get() > 0
    });
    wait_for_last_scene(&servo_test, &delegate, Duration::from_millis(20));
    webview.paint();

    let image = read(&servo_test, SIDE, SIDE);
    let inside = image.get_pixel(60, 60).0;
    let outside = image.get_pixel(400, 60).0;
    println!(
        "PROBE-FRAME dimensions={:?} scenes_annoncees={} dans_carre(60,60)={:?} hors_carre(400,60)={:?}",
        image.dimensions(),
        delegate.frames_ready.get(),
        inside,
        outside
    );
    // gpui stocke du BGRA (swap(0,2) au decodage) ; la question est ce que Servo rend vraiment.
    println!(
        "PROBE-FRAME ordre_des_octets={}",
        if inside[0] == 255 && inside[2] == 0 {
            "RGBA (swap 0<->2 necessaire avant gpui)"
        } else if inside[2] == 255 && inside[0] == 0 {
            "BGRA (directement compatible gpui)"
        } else {
            "inattendu"
        }
    );

    // Un defilement est le cas reel d'un panneau de navigateur : repaint complet, sans
    // re-layout ni changement de taille de surface. Il n'est applique qu'apres le
    // aller-retour `notify_input_event_handled` (`webview_renderer.rs:1201`), d'ou l'attente.
    let before = read(&servo_test, SIDE, SIDE).get_pixel(60, 60).0;
    let salie = scroll(&webview, &servo_test, &delegate, 200.0);
    webview.paint();
    let after = read(&servo_test, SIDE, SIDE).get_pixel(60, 60).0;
    println!(
        "PROBE-FRAME defilement avant={before:?} apres={after:?} frame_salie={salie} — {}",
        if before != after {
            "la page deroule, paint() mesure donc un vrai rendu"
        } else {
            "LA PAGE NE DEROULE PAS : les couts de paint() ci-dessous ne sont pas des couts de rendu"
        }
    );

    const FRAMES: u32 = 20;
    measure_frames(&servo_test, &webview, &delegate, SIDE, SIDE, FRAMES);

    // 500x500 est la taille du harnais, pas celle d'un panneau reel : le cout
    // par frame doit etre connu a une taille de vignette realiste avant D5.
    let (w, h) = (1280u32, 800u32);
    servo_test.rendering_context.resize(PhysicalSize::new(w, h));
    webview.resize(PhysicalSize::new(w, h));
    wait_for_last_scene(&servo_test, &delegate, Duration::from_millis(20));
    webview.paint();
    let resized = read(&servo_test, w as i32, h as i32);
    println!(
        "PROBE-FRAME apres_resize dimensions={:?} dans_carre(60,60)={:?}",
        resized.dimensions(),
        resized.get_pixel(60, 60).0
    );
    measure_frames(&servo_test, &webview, &delegate, w as i32, h as i32, FRAMES);

    assert_ne!(inside, outside, "la page ne rend rien de differentiable");
}

fn rect(w: i32, h: i32) -> DeviceIntRect {
    DeviceIntRect::new(
        Point2D::origin().cast_unit(),
        Point2D::new(w, h).cast_unit(),
    )
}

fn read(servo_test: &ServoTest, w: i32, h: i32) -> image::RgbaImage {
    servo_test
        .rendering_context
        .read_to_image(rect(w, h))
        .expect("software read_to_image should yield a frame")
}

/// Faire defiler la page de `dy` pixels vers le bas. `WheelDelta::y` est inverse du
/// deplacement de la vue (`webview_renderer.rs:1206`), d'ou le signe inverse ici.
/// Retourne `true` si une frame sale a bien ete annoncee apres le defilement.
fn scroll(
    webview: &WebView,
    servo_test: &ServoTest,
    delegate: &Rc<ProbeDelegate>,
    dy: f32,
) -> bool {
    let (frames, events) = (delegate.frames_ready.get(), delegate.events_handled.get());
    webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
        WheelDelta {
            x: 0.0,
            y: -dy as f64,
            z: 0.0,
            mode: WheelMode::DeltaPixel,
        },
        DevicePoint::new(SIDE as f32 / 2.0, SIDE as f32 / 2.0).into(),
    )));
    let traite = spin_until(servo_test, Duration::from_secs(2), || {
        delegate.events_handled.get() > events
    });
    traite
        && spin_until(servo_test, Duration::from_secs(2), || {
            delegate.frames_ready.get() > frames
        })
}

/// Etapes separees, dans le meme passage : le defilement coute un aller-retour
/// molette → thread script → thread peinture → scene, qui n'a rien a voir avec le cout du
/// rendu. Chronometrer les deux ensemble donne un chiffre independant de la taille de
/// l'image (verifie : ~20 ms a 500x500 comme a 1280x800), donc faux.
///
/// Trois essais : la dispersion d'une machine a 4 cœurs avec un seul essai est assez
/// grande (facteur ~3 observe) pour qu'un chiffre unique ne veuille rien dire.
///
/// `paint_sale` est le plancher : sans frame sale, `paint()` fait quand meme le clear
/// complet du contexte logiciel et l'appel WebRender. C'est ce que paie un embeddeur qui
/// re-rend une fenetre endommagee, pas ce que coute le rendu d'une page qui change.
fn measure_frames(
    servo_test: &ServoTest,
    webview: &WebView,
    delegate: &Rc<ProbeDelegate>,
    w: i32,
    h: i32,
    frames: u32,
) {
    for rep in 0..3 {
        let mut driver = Duration::ZERO;
        let mut paint = Duration::ZERO;
        let mut read_cost = Duration::ZERO;
        let mut frames_sales = 0u32;

        for _ in 0..frames {
            let start = Instant::now();
            if scroll(webview, servo_test, delegate, 20.0) {
                frames_sales += 1;
            }
            driver += start.elapsed();

            let start = Instant::now();
            webview.paint();
            paint += start.elapsed();

            let start = Instant::now();
            read(servo_test, w, h);
            read_cost += start.elapsed();

            servo_test.servo.spin_event_loop();
        }

        let clean_paint = {
            let start = Instant::now();
            for _ in 0..frames {
                webview.paint();
            }
            start.elapsed() / frames
        };

        let (driver, paint, read_cost) = (driver / frames, paint / frames, read_cost / frames);
        let exit = paint + read_cost;
        let mpx = w as f64 * h as f64 / 1e6;
        let rate = |cost: Duration| mpx * 1e9 / cost.as_nanos() as f64;
        println!(
            "PROBE-FRAME {w}x{h} essai{rep} : frames_sales={frames_sales}/{frames} | derouler {driver:?}, paint {paint:?} ({:.1} Mpx/s), paint_sale {clean_paint:?}, read {read_cost:?} ({:.1} Mpx/s), paint+read {exit:?} ({:.1} img/s)",
            rate(paint),
            rate(read_cost),
            1e9 / exit.as_nanos() as f64,
        );
    }
}

#[test]
fn probe_userscript_subframe_reach() {
    let servo_test = ServoTest::new();

    // Une origine distincte = un port distinct.
    let (foreign_server, foreign_url) =
        make_server(serve(page("CROSS_ORIGIN", "<title>foreign</title>")));
    let foreign = foreign_url.as_url().to_string();

    let top_body = page(
        "TOP_PAGE",
        &format!(
            "<iframe src=\"child.html\"></iframe>\
             <iframe srcdoc=\"<script>console.log('NATURAL|SRCDOC')</script><h1>src</h1>\"></iframe>\
             <iframe src=\"{foreign}\"></iframe>"
        ),
    );
    let child_body = page("SAME_ORIGIN", "<title>child</title>");

    let (top_server, top_url) = make_server(
        move |request: http::Request<Incoming>, response: &mut HtmlResponse| {
            response
                .headers_mut()
                .insert(CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
            let body = if request.uri().path().ends_with("child.html") {
                child_body.clone()
            } else {
                top_body.clone()
            };
            *response.body_mut() = make_body(body);
        },
    );
    let top_href = top_url.as_url().to_string();

    let manager = Rc::new(UserContentManager::new(servo_test.servo()));
    manager.add_script(Rc::new(UserScript::from(
        "console.log('EGOMARK|' + (window.top === window ? 'TOP' : 'CHILD') + '|' + location.href)",
    )));
    // Second temps de la sonde : ce que le `UserScript` n'atteint pas, le frame principal
    // peut-il l'atteindre depuis `contentWindow` ? C'est la seule issue si l'injection
    // multi-frames doit passer par un script utilisateur.
    manager.add_script(Rc::new(UserScript::from(
        r#"
        addEventListener('load', () => {
            console.log('VIA_TOP|TOP_CHARGE|' + document.querySelectorAll('iframe').length);
            document.querySelectorAll('iframe').forEach((frame, index) => {
                try {
                    frame.contentWindow.eval("console.log('VIA_TOP|DANS_ENFANT|' + location.href)");
                } catch (error) {
                    console.log('VIA_TOP|BLOQUE|' + index + '|' + error.name);
                }
            });
        });
        "#,
    )));

    let delegate = Rc::<WebViewDelegateImpl>::default();
    let webview = WebViewBuilder::new(servo_test.servo(), servo_test.rendering_context.clone())
        .user_content_manager(manager)
        .url(top_url.as_url().clone())
        .delegate(delegate.clone())
        .build();

    servo_test.spin(|| webview.load_status() != LoadStatus::Complete);

    let collect = |prefix: &str| -> Vec<String> {
        delegate
            .console_messages
            .borrow()
            .iter()
            .map(|(_, message)| message.clone())
            .filter(|message| message.starts_with(prefix))
            .collect()
    };

    // On attend que les QUATRE frames se soient signalees d'elles-memes ; si l'une
    // d'elles manque a la fin, c'est qu'elle n'a pas charge, et son absence de
    // EGOMARK ne voudrait rien dire. Puis on attend que le `load` du frame principal
    // ait parcouru ses sous-frames.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && collect("NATURAL|").len() < 4 {
        servo_test.servo.spin_event_loop();
        thread::sleep(Duration::from_millis(5));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && collect("VIA_TOP|TOP_CHARGE").is_empty() {
        servo_test.servo.spin_event_loop();
        thread::sleep(Duration::from_millis(5));
    }
    servo_test.servo.spin_event_loop();

    let naturals = collect("NATURAL|");
    let egos = collect("EGOMARK|");
    let via_top = collect("VIA_TOP|");
    let seen = |list: &Vec<String>, needle: &str| list.iter().any(|mark| mark.contains(needle));
    for (frame, natural_needle, ego_needle, top_needle) in [
        ("top", "TOP_PAGE", top_href.as_str(), "TOP_CHARGE"),
        ("meme_origine", "SAME_ORIGIN", "child.html", "child.html"),
        ("srcdoc", "SRCDOC", "about:srcdoc", "about:srcdoc"),
        (
            "origine_etrangere",
            "CROSS_ORIGIN",
            foreign.as_str(),
            foreign.as_str(),
        ),
    ] {
        println!(
            "PROBE-FRAMES {frame}: charge={} userscript={} depuis_le_top={}",
            seen(&naturals, natural_needle),
            seen(&egos, ego_needle),
            seen(&via_top, top_needle),
        );
    }
    for mark in naturals.iter().chain(egos.iter()).chain(via_top.iter()) {
        println!("PROBE-FRAMES  {mark}");
    }

    top_server.close();
    foreign_server.close();
}
