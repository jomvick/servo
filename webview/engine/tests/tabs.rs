//! Une page qui rend, une navigation qui marche, un `resize(0, 0)` qui ne casse rien.
//!
//! Un seul `#[test]` ici, et ce n'est pas une préférence : `Opts` est un singleton de
//! processus (`components/config/opts.rs:279`), donc deux `Engine` dans le même binaire
//! paniquent au second `Servo::new`. Les scénarios s'enchaînent dans la même instance.

use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbaImage;
use webview_core::{ProfileId, TabId};
use webview_engine::{Engine, EngineConfig, EngineEvent};

const VIEWPORT: (u32, u32) = (800, 600);
/// Ce délai n'est pas un budget de latence : le moteur rend en ~0,3 s quand il est chaud. C'est
/// le plafond du premier démarrage d'un binaire de debug de ~200 Mo sur cette machine (7,8 Go de
/// RAM, quatre cœurs, souvent occupée par un build komet) — observé une fois à plus de 60 s
/// d'attente, zéro événement, puis huit runs d'affilée à 0,3 s. Mesurer la pagination du disque
/// sous le nom du moteur produirait un faux échec.
const DEADLINE: Duration = Duration::from_secs(180);

/// Tous les événements vus depuis le début. `take_events` vide la file entière, donc les
/// aides doivent garder ce qu'elles ne cherchent pas : le titre arrive pendant l'attente
/// d'une frame, et le jeter ferait croire qu'il n'est jamais venu.
#[derive(Default)]
struct Seen {
    events: Vec<EngineEvent>,
}

fn kind(event: &EngineEvent) -> &'static str {
    match event {
        EngineEvent::Frame { .. } => "frame",
        EngineEvent::UrlChanged { .. } => "url",
        EngineEvent::TitleChanged { .. } => "titre",
        EngineEvent::LoadStatusChanged { .. } => "chargement",
        EngineEvent::Closed { .. } => "fermé",
        EngineEvent::Crashed { .. } => "crash",
    }
}

impl Seen {
    fn pump(&mut self, engine: &mut Engine) {
        engine.pump();
        self.events.extend(engine.take_events());
    }

    /// La prochaine frame de `tab` qui satisfait `wanted`, dans la limite du délai. Un
    /// chargement rend d'abord une frame blanche (D5 : le contenu n'est pas encore dans la
    /// scène) ; attendre un contenu précis est la seule façon stable de savoir qu'il est là.
    fn wait_frame(
        &mut self,
        engine: &mut Engine,
        tab: TabId,
        what: &str,
        wanted: impl Fn(&RgbaImage) -> bool,
    ) -> Arc<RgbaImage> {
        let started = Instant::now();
        let mut next = self.events.len();
        loop {
            self.pump(engine);
            while next < self.events.len() {
                if let EngineEvent::Frame { tab: other, image } = &self.events[next]
                    && *other == tab
                    && wanted(image)
                {
                    return Arc::clone(image);
                }
                next += 1;
            }
            let since: Vec<&str> = self.events.iter().map(kind).collect();
            assert!(
                started.elapsed() < DEADLINE,
                "{what} jamais rendu en {DEADLINE:?} ; événements depuis le début : {since:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Attend que le dernier titre annoncé pour `tab` soit `wanted`. Le titre est un état
    /// (le moteur ne renvoie que les changements), donc un titre déjà vu et non dépassé
    /// reste la réponse valable ; c'est un titre plus récent qui le rend caduc.
    fn wait_title(&mut self, engine: &mut Engine, tab: TabId, wanted: &str) {
        let started = Instant::now();
        loop {
            self.pump(engine);
            let last = self.events.iter().rev().find_map(|event| match event {
                EngineEvent::TitleChanged { tab: other, title } if *other == tab => {
                    Some(title.as_str())
                },
                _ => None,
            });
            if last == Some(wanted) {
                return;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "le titre `{wanted}` n'est jamais arrivé ; dernier titre vu : {last:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn engine_boots_renders_navigates_and_survives_a_zero_resize() {
    let mut engine = Engine::new(EngineConfig {
        viewport: VIEWPORT,
        ..EngineConfig::default()
    })
    .expect("moteur");
    let space = engine.create_space("phase1", ProfileId("default".into()));

    let page = |colour: &str, title: &str| {
        format!(
            "data:text/html,<title>{title}</title><style>body{{margin:0;background:%23{colour}}}</style><h1>x</h1>"
        )
    };

    let start = page("ff0000", "one");
    let tab = engine.create_tab(space, Some(&start)).expect("onglet");
    let mut seen = Seen::default();
    let frame = seen.wait_frame(&mut engine, tab, "le rouge 800x600", |image| {
        let px = image.get_pixel(10, 10).0;
        image.dimensions() == VIEWPORT && px[0] > 200 && px[1] < 60
    });
    assert_eq!((frame.width(), frame.height()), (VIEWPORT.0, VIEWPORT.1));
    assert!(
        frame.get_pixel(10, 10).0[1] < 60,
        "le rouge attendu n'est pas là : {:?}",
        frame.get_pixel(10, 10).0
    );
    seen.wait_title(&mut engine, tab, "one");

    // Navigation, puis retour : le modèle suit ce que le moteur annonce.
    engine.navigate(tab, &page("00ff00", "two")).unwrap();
    seen.wait_title(&mut engine, tab, "two");
    seen.wait_frame(&mut engine, tab, "le vert", |image| {
        image.get_pixel(10, 10).0[1] > 200
    });
    engine.go_back(tab).unwrap();
    seen.wait_title(&mut engine, tab, "one");

    // `resize(0, 0)` est ignoré, la taille précédente reste rendue.
    let before = seen.wait_frame(&mut engine, tab, "la frame d'avant le resize", |image| {
        image.dimensions() == VIEWPORT
    });
    engine.resize(tab, 0, 0).unwrap();
    engine.pump();
    assert_eq!((before.width(), before.height()), (VIEWPORT.0, VIEWPORT.1));

    engine.resize(tab, 400, 300).unwrap();
    seen.wait_frame(&mut engine, tab, "la frame 400x300", |image| {
        image.dimensions() == (400, 300)
    });

    engine.close_tab(tab).unwrap();
    assert!(engine.registry().tab(tab).is_err());
}
