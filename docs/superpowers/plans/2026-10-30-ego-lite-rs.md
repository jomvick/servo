# ego-lite sur fork Servo — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Construire ego-lite (navigateur pour agents : Spaces parallèles, snapshot de page, API `ego.*`) en modifiant directement un fork de Servo, avec une UI gpui (stack komet), Linux d'abord, intégrable plus tard comme panneau navigateur dans komet.

**Architecture:** Le dépôt est le clone de Servo (`browser/`, HEAD `0626de7b4e`), sur une branche `ego-lite`. Les nouvelles crates vivent dans `ego/` et sont membres du workspace Servo, donc elles consomment `components/servo` par chemin avec le `Cargo.lock` et les profils de build de Servo. Servo fournit déjà la persistance de profil (`Opts::config_dir` → `cookie_jar.json`, `webstorage.sqlite`) et l'injection de scripts dans les frames (`UserContentManager`) : le fork sert de base et de point d'extension, v1 n'ajoute qu'un flag CLI côté servoshell. Tout le reste (Spaces, snapshot, actions, pont, UI gpui) est du code neuf dans `ego/`.

**Tech Stack:** Rust 1.99.1 (toolchain de Servo), edition 2024, `components/servo` (path), `accesskit`, `gpui` + `gpui-base` (rev komet `e2ddcc6`), `tokio`, `serde`, `thiserror`, `tracing`, `clap`, `image 0.25`, `tokio-tungstenite`, `rusqlite`.

**Spec:** `docs/superpowers/plans/…` (ce fichier) + `docs/adr/0001-architecture.md` (créé tâche 1 : décisions D1–D8, périmètre v1, ce qui est repris d'ego et ce qui est abandonné).

## Global Constraints

- Base : clone de `https://github.com/servo/servo` à `0626de7b4e`, branche de travail `ego-lite`. `origin` = le fork de l'utilisateur (ajouté quand il existe), `upstream` = servo/servo.
- Toolchain : celle de Servo (`1.99.1`), `rust-toolchain.toml` inchangé. Ne jamais bumper sans re-tester `cargo build -p servoshell`.
- Nouveaux fichiers dans `ego/` sous MPL-2.0 (cohérence du fork) ; `LICENSE` racine inchangé.
- `gpui` et `gpui-base` épinglés à la **même révision que komet** (`rev = "e2ddcc6805f8c5088e62a60dfe517abcccd61a9a"`), déclarés dans `[workspace.dependencies]` de Servo. Aucune autre révision.
- Linux x86_64 = cible de référence. Code spécifique plateforme derrière `#[cfg(target_os = …)]` ; macOS/Windows doivent compiler.
- Toute erreur exposée porte un code `EGO_*` de la liste publiée par ego (`docs/native-bindings-api.md`, upstream ego-lite) : `EGO_TASK_SPACE_NOT_FOUND`, `EGO_TASK_SPACE_USER_IN_CONTROL`, `EGO_WEB_CONTENTS_UNAVAILABLE`, `EGO_SNAPSHOT_FAILED`, `EGO_INVALID_ARGUMENT`, `EGO_TASK_HOST_DISCONNECTED`, `EGO_OPERATION_FAILED`, `EGO_CDP_CHANNEL_UNAVAILABLE`.
- Noms publics identiques à ego : `snapshot`, `createTab`, `listTabs`, `useTaskSpace`, `listTaskSpaces`, `createTaskSpace`, `claimTaskSpace`, `handOffTaskSpace`, `takeOverTaskSpace`, `completeTaskSpace`, `closeTaskSpace` (camelCase JSON/JS, snake_case Rust).
- Un seul thread pour Servo, le `Registry` et gpui (aucun des trois n'est `Send`). Le pont réseau vit sur un thread tokio séparé.
- Chaque tâche se termine par `cargo fmt --all --check && cargo clippy -p <crate> -- -D warnings && cargo test -p <crate>`, puis un commit Conventional Commits.
- Pas de binaires CEF/Chromium : le moteur est Servo, point.

## Review Focus

1. **iframe cross-origin** : son contenu doit apparaître dans le snapshot, ou une ligne `iframe "…" [unreadable]` explicite — jamais une omission silencieuse. Test tâche 3.
2. **iframe qui ne répond jamais** (pub coincée) : le snapshot se termine dans le budget (1,5 s) avec un marqueur `[frame timeout]`, il ne bloque pas l'agent. Test tâche 3.
3. **Action agent pendant que l'utilisateur a la main** : échec `EGO_TASK_SPACE_USER_IN_CONTROL` sans mutation de l'onglet. Tests tâches 2 et 7.
4. **Onglet fermé ou WebView crashé en cours de requête** : la requête pendante échoue avec `EGO_WEB_CONTENTS_UNAVAILABLE` dans les 2 s, jamais de blocage. Test tâche 7.
5. **Viewport 0×0 / fenêtre minimisée** : pas de panique ni d'allocation géante, dernière frame valide conservée. Test tâche 4.
6. **Pont local** : écoute uniquement sur `127.0.0.1`, token obligatoire, rejet immédiat sinon. Test tâche 7.
7. **Fichier de profil corrompu ou tronqué** (cookies/localStorage) : le démarrage réussit, le fichier est ignoré et réécrit. Test tâche 6.

---

## Structure des fichiers

```
browser/                              # clone servo, branche ego-lite
  Cargo.toml                          # + members "ego/core","ego/engine","ego/bridge","ego/ui","ego/cli" ; + gpui/gpui-base
  ports/servoshell/desktop/cli.rs     # MODIFIÉ (optionnel) : flag --ego-data-dir pour tester à la main
  ego/
    core/src/{lib,ids,error,space,tab,snapshot}.rs
    core/tests/fixtures/snapshot/*.json
    engine/src/{lib,host,tabs,preload,input,command,delegate,config}.rs
    engine/tests/{boot,tabs,snapshot,actions,persist}.rs
    bridge/src/{lib,protocol,dispatch}.rs
    bridge/assets/ego-preload.js       # script injecté dans toutes les frames
    bridge/tests/rpc.rs
    ui/src/{lib,app,browser_view,view_model,frame_loop}.rs
    cli/src/main.rs                    # binaires ego-lite
  docs/adr/0001-architecture.md
```

---

### Task 1: Fork opérationnel — branche, crates ego dans le workspace, gpui vérifié

**Files:**
- Modify: `Cargo.toml` (racine Servo : `members`, `[workspace.dependencies]`)
- Create: `ego/core/Cargo.toml`, `ego/core/src/lib.rs`, `ego/engine/Cargo.toml`, `ego/engine/src/lib.rs`, `ego/bridge/Cargo.toml`, `ego/bridge/src/lib.rs`, `ego/ui/Cargo.toml`, `ego/ui/src/lib.rs`, `ego/cli/Cargo.toml`, `ego/cli/src/main.rs`
- Create: `docs/adr/0001-architecture.md`
- Test: `ego/core/tests/workspace.rs`

**Interfaces:**
- Produces: workspace Servo compilant avec les cinq crates ; `ego_core::CRATE_VERSION` ; `ego_ui` compilant avec gpui sous le toolchain Servo.

- [ ] **Step 1: Écrire le test de fumée**

```rust
// ego/core/tests/workspace.rs
#[test]
fn workspace_exposes_version() {
    assert!(!ego_core::CRATE_VERSION.is_empty());
}
```

- [ ] **Step 2: Configurer git et le workspace**

```bash
git switch -c ego-lite
git remote rename origin upstream
```

Créer les manifestes (`ego/core/src/lib.rs` : `pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");`). Ajouter à la racine Servo : `members` += `"ego/core", "ego/engine", "ego/bridge", "ego/ui", "ego/cli"` ; `[workspace.dependencies]` += `gpui`, `gpui-base` (rev komet), `image`, `smallvec`, `tokio-tungstenite`, `thiserror`, `rusqlite`. Ne pas dupliquer une dépendance déjà présente dans `[workspace.dependencies]` de Servo.

- [ ] **Step 3: Vérifier la combinaison toolchain Servo 1.99.1 × gpui komet**

Run: `cargo check -p ego-ui`
Expected: PASS. Repli documenté dans l'ADR si échec : déplacer `ego/` dans un workspace séparé avec `[patch.crates-io]` recopié de la racine Servo, et conserver `components/servo` en dépendance chemin.

- [ ] **Step 4: Écrire l'ADR et vérifier le build de référence**

`docs/adr/0001-architecture.md` : D1 fork Servo plutôt que crate crates.io (base git, accès au moteur pour les évolutions) ; D2 snapshot par walker JS injecté dans toutes les frames + agrégation `postMessage` (pas de patch profond de `script`) ; D3 pont `globalThis.ego` par polling toutes les 8–16 ms ; D4 persistance profil fournie par `Opts::config_dir` (`cookie_jar.json` + `webstorage.sqlite`) exposée en `--data-dir` ; D5 UI gpui embarquée ; D6 CDP limité à `ego.evaluate` + sous-ensemble devtools ; D7 thread principal unique ; D8 périmètre v1 et reports (extensions, téléchargements, favoris, migration Chrome, sync).

Run: `cargo build -p servoshell && cargo test -p ego-core --test workspace`
Expected: PASS.

- [ ] **Step 5: Committer**

```bash
git add -A && git commit -m "chore(ego): add ego crates to the servo workspace"
```

---

### Task 2: Modèle pur — Spaces, onglets, propriété

**Files:**
- Create: `ego/core/src/{ids.rs,error.rs,space.rs,tab.rs}`, `ego/core/tests/spaces.rs`
- Modify: `ego/core/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct SpaceId(pub u64); pub struct TabId(pub u64); pub struct ProfileId(pub String);`
  - `pub enum Ownership { Agent, User, Done }`
  - `pub struct Space { pub id, pub name: String, pub profile: ProfileId, pub tabs: Vec<TabId>, pub active: Option<TabId>, pub ownership: Ownership }`
  - `pub struct Tab { pub id, pub space: SpaceId, pub url: Option<String>, pub title: Option<String>, pub loading: bool }`
  - `pub struct Registry`
  - `impl Registry { create_space(name, ProfileId) -> SpaceId; space(SpaceId) -> Result<&Space, EgoError>; tab(TabId) -> Result<&Tab, EgoError>; create_tab(SpaceId, Option<String>) -> Result<TabId, EgoError>; claim(SpaceId) -> Result<(), EgoError>; hand_off(SpaceId) -> Result<(), EgoError>; take_over(SpaceId) -> Result<(), EgoError>; complete(SpaceId) -> Result<(), EgoError>; close(SpaceId) -> Result<Vec<TabId>, EgoError>; require_agent_control(SpaceId) -> Result<&Space, EgoError> }`
  - `pub struct EgoError { pub code: ErrorCode, pub message: String }` avec `ErrorCode` sérialisé en `EGO_*`.

- [ ] **Step 1: Écrire les tests**

```rust
#[test]
fn hand_off_blocks_agent_mutations() {
    let mut r = Registry::default();
    let s = r.create_space("checkout", ProfileId("default".into()));
    r.claim(s).unwrap();
    r.hand_off(s).unwrap();
    assert_eq!(r.create_tab(s, None).unwrap_err().code, ErrorCode::TaskSpaceUserInControl);
    assert!(r.space(s).unwrap().tabs.is_empty());
}

#[test]
fn take_over_and_complete_restore_control() {
    let mut r = Registry::default();
    let s = r.create_space("scrape", ProfileId("default".into()));
    r.claim(s).unwrap();
    r.hand_off(s).unwrap();
    r.take_over(s).unwrap();
    assert!(r.create_tab(s, None).is_ok());
    r.complete(s).unwrap();
    assert!(r.create_tab(s, None).is_err());
}

#[test]
fn close_removes_tabs_and_unknown_ids_are_reported() {
    let mut r = Registry::default();
    let s = r.create_space("x", ProfileId("default".into()));
    let t = r.create_tab(s, None).unwrap();
    assert_eq!(r.close(s).unwrap(), vec![t]);
    assert_eq!(r.tab(t).unwrap_err().code, ErrorCode::WebContentsUnavailable);
    assert_eq!(r.space(SpaceId(99)).unwrap_err().code, ErrorCode::TaskSpaceNotFound);
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-core --test spaces`
Expected: FAIL, modules absents.

- [ ] **Step 3: Implémenter**

`Done` est terminal pour les ops agent (`EGO_TASK_SPACE_INACTIVE`) ; `create_tab` exige `Ownership::Agent` ; `hand_off` depuis `User` est idempotent ; `close` supprime tous les onglets du space.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-core --test spaces`
Expected: PASS (3 tests).

- [ ] **Step 5: Committer**

```bash
git add ego/core && git commit -m "feat(core): space, tab and ownership state machine"
```

---

### Task 3: Snapshot multi-frames par script injecté

**Files:**
- Create: `ego/bridge/assets/ego-preload.js` (partie snapshot), `ego/core/src/snapshot.rs`, `ego/core/tests/snapshot.rs`, `ego/core/tests/fixtures/snapshot/{form,iframe,hung}.json`
- Create: `ego/engine/src/preload.rs`, `ego/engine/tests/snapshot.rs`
- Modify: `ego/core/src/lib.rs`

**Interfaces:**
- Consumes: `servo::UserContentManager` + `UserScript` (injecté dans toutes les frames), `WebView::evaluate_javascript`.
- Produces:
  - `pub struct SnapshotOptions { pub max_chars: usize, pub interactive_only: bool }` (`Default` : 12_000, false)
  - `pub struct RefEntry { pub reference: String, pub role: String, pub name: String, pub frame: String }` (`reference` = `"f0:e12"`)
  - `pub struct Snapshot { pub title: String, pub url: String, pub text: String, pub refs: Vec<RefEntry> }`
  - `pub fn parse_snapshot(raw: &serde_json::Value, options: &SnapshotOptions) -> Result<Snapshot, EgoError>`
  - `Engine::install_preload(&mut self)`, `Engine::snapshot(TabId, &SnapshotOptions) -> Result<Snapshot, EgoError>`

- [ ] **Step 1: Écrire les tests du parseur (fixtures = JSON produit par le JS)**

```rust
#[test]
fn frame_tree_flattens_nested_documents() {
    let raw = fixture("iframe");
    let snap = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert!(snap.text.contains("iframe \"Checkout\""));
    assert!(snap.text.contains("Pay now"));
    assert!(snap.refs.iter().any(|r| r.reference.starts_with("f1:")));
}

#[test]
fn unresponsive_frame_is_marked_not_blocking() {
    let snap = parse_snapshot(&fixture("hung"), &SnapshotOptions::default()).unwrap();
    assert!(snap.text.contains("[frame timeout]"));
    assert!(snap.text.lines().count() < 200);
}

#[test]
fn text_is_truncated_to_budget() {
    let opts = SnapshotOptions { max_chars: 100, ..Default::default() };
    assert!(parse_snapshot(&fixture("form"), &opts).unwrap().text.chars().count() <= 164);
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-core --test snapshot`
Expected: FAIL.

- [ ] **Step 3: Écrire le walker JS**

`ego-preload.js` (IIFE, exécuté dans chaque frame) :
- rôle déduit de la balise/ARIA (`a`→link, `button`→button, `input[type]`→textbox/checkbox/radio, `select`→combobox, `h1..h6`→heading, `img[alt]`→image, `iframe`→iframe) ;
- texte visible des feuilles, tronqué à 120 caractères ;
- éléments actionnables taggés `data-ego-ref="f{frameId}:e{n}"`, bounds via `getBoundingClientRect()` ;
- registre de frames : chaque frame envoie `{type:"ego-hello", frameId}` à `window.top` ; le top tient la liste (id + `WindowProxy`) ;
- `__egoSnapshot(opts)` dans le top : demande récursivement (`postMessage`) à chaque frame, attend max 500 ms par frame, marque les retardataires `[frame timeout]`, agrège et renvoie un JSON `{title, url, frames:[{id, path, nodes:[…]}]}` ;
- `window.__egoTake()` (file d'appels agents) et `window.__egoSettle(id, ok, json)`.
Le tout est concaténé dans `EGO_PRELOAD_JS` via `include_str!`.

- [ ] **Step 4: Implémenter le parseur et le test live**

`parse_snapshot` aplatit les frames dans l'ordre, préfixe les réfs par le chemin de frame, applique `max_chars`, et produit le texte : `# {title}\n{url}\n` puis `- {role} "{name}"[ [ref=…]][ value="…"]` indenté de 2 espaces par niveau. `Engine::install_preload` ajoute le script au `UserContentManager` partagé avant le premier chargement. Test live : page `iframe srcdoc` + iframe `data:`, `snapshot` contient les deux ; iframe qui ne répond pas → `[frame timeout]` en < 2 s. Si Servo n'injecte pas `UserScript` dans les iframes, repli documenté : évaluer le walker dans chaque `WebView` d'iframe (une WebView par iframe n'existe pas ici) → escalade en patch `components/script` (hors v1, l'ADR note le coût).

Run: `cargo test -p ego-core --test snapshot && cargo test -p ego-engine --test snapshot`
Expected: PASS.

- [ ] **Step 5: Committer**

```bash
git add ego/core ego/engine ego/bridge && git commit -m "feat(snapshot): cross-frame ego snapshot via injected walker"
```

---

### Task 4: Onglets et frames dans `ego/engine`

**Files:**
- Create: `ego/engine/src/{lib.rs,host.rs,tabs.rs,delegate.rs,command.rs,waker.rs}`, `ego/engine/tests/{boot,tabs}.rs`
- Modify: `ego/engine/Cargo.toml` (feature `servo/default_web_features` non requise ; dépendre de `servo` path)

**Interfaces:**
- Consumes: `ego_core::Registry`, `servo::{Servo, ServoBuilder, WebView, WebViewBuilder, SoftwareRenderingContext, RgbaImage}`.
- Produces:
  - `EngineConfig { data_dir: PathBuf, viewport: (u32, u32) }`
  - `Engine::new(EngineConfig) -> Result<Engine, EgoError>`, `Engine::pump(&mut self)`, `Engine::registry(&self)`, `Engine::registry_mut(&mut self)`
  - `Engine::create_tab(SpaceId, Option<&str>) -> Result<TabId, EgoError>`, `navigate`, `reload`, `back`, `forward`, `close_tab`, `resize(TabId, u32, u32)`
  - `EngineEvent::{Frame{tab, image: Arc<RgbaImage>}, UrlChanged, TitleChanged, LoadStatusChanged, OwnershipChanged, Closed, Crashed}`
  - `Engine::input(TabId, servo::InputEvent)`, `Engine::click_at(TabId, f64, f64)`

- [ ] **Step 1: Écrire les tests**

```rust
#[test]
fn boots_renders_and_navigates() {
    let (mut e, _h) = test_engine();
    let s = new_space(&mut e);
    let t = e.create_tab(s, Some("data:text/html,<title>one</title><h1>x</h1>")).unwrap();
    let frame = wait_frame(&mut e);
    assert_eq!(frame.dimensions(), (800, 600));
    assert!(frame.pixels().any(|p| p.0[0] < 250));
    wait_title(&mut e, "one");
    e.navigate(t, "data:text/html,<title>two</title>");
    wait_title(&mut e, "two");
    e.back(t);
    wait_title(&mut e, "one");
}

#[test]
fn zero_sized_resize_is_safe() {
    let (mut e, _h) = test_engine();
    let t = e.create_tab(new_space(&mut e), Some("data:text/html,<h1>x</h1>")).unwrap();
    let before = wait_frame(&mut e);
    e.resize(t, 0, 0);
    e.pump();
    e.resize(t, 400, 300);
    assert_eq!(wait_frame(&mut e).dimensions(), (400, 300));
    assert_eq!(before.dimensions(), (800, 600));
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-engine --test boot --test tabs`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

`Engine` sur le thread appelant : `SoftwareRenderingContext::new(PhysicalSize)` + `make_current()` par onglet, `ServoBuilder::default().event_loop_waker(Waker(Arc<AtomicBool>))` une fois, un `WebView` par onglet relié au même `UserContentManager`. `Delegate::notify_new_frame_ready` → `paint()` → `read_to_image` → `EngineEvent::Frame`. `pump()` : `servo.spin_event_loop()`, draine les commandes et événements. `resize(0,0)` conserve la dernière taille (pas de contexte 0×0). Le delegate traduit URL, titre, statut de chargement, fermeture, crash.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-engine --test boot --test tabs`
Expected: PASS.

- [ ] **Step 5: Committer**

```bash
git add ego/engine && git commit -m "feat(engine): servo-backed tabs, frames and navigation"
```

---

### Task 5: Actions par référence

**Files:**
- Create: `ego/engine/src/input.rs`, `ego/engine/tests/actions.rs`, `ego/engine/tests/fixtures/form.html`
- Modify: `ego/engine/src/lib.rs`, `ego/bridge/assets/ego-preload.js`

**Interfaces:**
- Consumes: `Snapshot::refs`, `Engine::snapshot`, `Engine::evaluate`, `Engine::input`.
- Produces: `Engine::click(TabId, &str)`, `Engine::fill(TabId, &str, &str)`, `Engine::type_text(TabId, &str)`, `Engine::scroll(TabId, f64, f64)`, `Engine::evaluate(TabId, &str) -> Result<serde_json::Value, EgoError>`.

- [ ] **Step 1: Écrire les tests**

```rust
#[test]
fn click_by_ref_runs_the_handler() {
    let (mut e, _h) = test_engine();
    let t = open_form(&mut e);
    let save = ref_named(&mut e, t, "Save");
    e.click(t, &save).unwrap();
    assert_eq!(e.evaluate(t, "document.title").unwrap(), serde_json::json!("saved"));
}

#[test]
fn fill_sets_value_and_fires_input() {
    let (mut e, _h) = test_engine();
    let t = open_form(&mut e);
    let email = ref_role(&mut e, t, "textbox");
    e.fill(t, &email, "a@b.c").unwrap();
    assert_eq!(e.evaluate(t, "document.querySelector('input').dataset.typed").unwrap(), serde_json::json!("1"));
    assert_eq!(e.evaluate(t, "document.querySelector('input').value").unwrap(), serde_json::json!("a@b.c"));
}

#[test]
fn trusted_click_falls_back_to_coordinates() {
    // fixture : bouton qui vérifie event.isTrusted et écrit dans le titre
    assert_eq!(e.evaluate(t, "document.title").unwrap(), serde_json::json!("trusted"));
}

#[test]
fn unknown_ref_is_invalid_argument() {
    let (mut e, _h) = test_engine();
    let t = open_form(&mut e);
    assert_eq!(e.click(t, "f0:e999").unwrap_err().code, ErrorCode::InvalidArgument);
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-engine --test actions`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

Chemin nominal : action JS routée vers la frame propriétaire de la réf (`element.click()`, focus + setter natif + `input`/`change` pour `fill`, `element.scrollIntoView()` pour `scroll`). Si le gestionnaire exige `isTrusted`, basculer sur `InputEvent` Servo au centre des bounds mémorisés dans `RefEntry`. Le walker stocke les bounds dans une map `data-ego-bounds` lisible par la frame. Réf inconnue → `EGO_INVALID_ARGUMENT`.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-engine --test actions`
Expected: PASS (4 tests).

- [ ] **Step 5: Committer**

```bash
git add ego/engine ego/bridge && git commit -m "feat(engine): ref-based click, fill and trusted-event fallback"
```

---

### Task 6: Profil persistant (`config_dir` Servo) et test de redémarrage

**Files:**
- Create: `ego/engine/src/config.rs`, `ego/engine/tests/persist.rs`
- Modify: `ego/engine/src/{lib.rs,host.rs}`, `ego/cli/src/main.rs`, `ports/servoshell/desktop/cli.rs` (optionnel)

**Interfaces:**
- Consumes: `servo_config::opts::Opts { config_dir: Option<PathBuf>, .. }` (existant), `ServoBuilder::opts(...)`, `components/net/resource_thread.rs` (écrit `cookie_jar.json`), `components/storage/webstorage/engines/sqlite.rs` (écrit `webstorage.sqlite`).
- Produces: `EngineConfig { data_dir, viewport }` avec `data_dir` → `Opts::config_dir` ; `ego-lite --data-dir <PATH>` (défaut `$XDG_DATA_HOME/ego-lite`) ; `Engine::data_dir(&self) -> &Path`.

- [ ] **Step 1: Écrire les tests** (si un second `Servo` ne peut pas être recréé dans le même processus, remplacer la seconde moitié par un sous-processus `ego-lite headless` + `ego-lite call`)

```rust
// ego/engine/tests/persist.rs
#[test]
fn cookies_and_local_storage_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut e = Engine::new(cfg(dir.path())).unwrap();
        let t = e.create_tab(new_space(&mut e), Some("data:text/html,<script>document.cookie='session=abc; path=/'; localStorage.setItem('k','v')</script>")).unwrap();
        wait_load(&mut e, t);
        drop(e);
    }
    assert!(dir.path().join("cookie_jar.json").exists());
    assert!(dir.path().join("webstorage.sqlite").exists());
    let mut e = Engine::new(cfg(dir.path())).unwrap();
    let t = e.create_tab(new_space(&mut e), Some("data:text/html,<body>")).unwrap();
    wait_load(&mut e, t);
    assert_eq!(e.evaluate(t, "localStorage.getItem('k')").unwrap(), serde_json::json!("v"));
    assert!(e.evaluate(t, "document.cookie").unwrap().as_str().unwrap().contains("session=abc"));
}

#[test]
fn corrupt_profile_files_do_not_block_startup() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cookie_jar.json"), b"{not json").unwrap();
    std::fs::write(dir.path().join("webstorage.sqlite"), b"garbage").unwrap();
    let mut e = Engine::new(cfg(dir.path())).unwrap();
    let t = e.create_tab(new_space(&mut e), Some("data:text/html,<h1>ok</h1>")).unwrap();
    wait_load(&mut e, t);
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-engine --test persist -- --nocapture`
Expected: FAIL (`cookie_jar.json` absent : `config_dir` vaut `None`).

- [ ] **Step 3: Câbler `config_dir`**

`Engine::new` construit `Opts { config_dir: Some(config.data_dir.clone()), ..Opts::default() }` et le passe à `ServoBuilder::opts(opts)`. `EngineConfig::default_data_dir()` = `dirs::data_dir().join("ego-lite")`, créé avec `create_dir_all`. Rien à patcher dans `components/net` ni `components/storage` : `resource_thread` écrit déjà `cookie_jar.json` (et `auth_cache.json`, `hsts_list.json`) et `SqliteEngine` écrit `webstorage.sqlite`. Ajouter le flag `--data-dir` à `ego-lite` et, pour les tests manuels, `--ego-data-dir` à servoshell (`Opts.config_dir`). Vérifier que `read_json_from_file` ignore un JSON invalide (sinon l'entourer d'un `if let Ok`).

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-engine --test persist`
Expected: PASS (2 tests).

- [ ] **Step 5: Committer**

```bash
git add ego ports/servoshell && git commit -m "feat(engine): persistent profile via servo config_dir"
```

---

### Task 7: Pont JSON-RPC et CLI headless

**Files:**
- Create: `ego/bridge/src/{protocol.rs,dispatch.rs}`, `ego/bridge/tests/rpc.rs`, `ego/cli/src/main.rs`
- Modify: `ego/engine/src/{command.rs,host.rs}`

**Interfaces:**
- Produces: `BridgeInfo { port: u16, token: String }`, `serve(SocketAddr, EngineHandle) -> Result<BridgeInfo, EgoError>` (écrit `{data_dir}/bridge.json`, mode 0600) ; protocole JSON-RPC 2.0 `{"jsonrpc":"2.0","id":1,"method":"ego.snapshot","params":{"tab":3}}` ; méthodes v1 : `ego.getBrowserVersion`, `listProfiles`, `listTaskSpaces`, `createTaskSpace`, `claimTaskSpace`, `closeTaskSpace`, `completeTaskSpace`, `handOffTaskSpace`, `takeOverTaskSpace`, `listTabs`, `createTab`, `closeTab`, `navigate`, `snapshot`, `evaluate`, `click`, `fill`, `type`, `scroll`, `capture`, `sendCDPMessage` (→ `EGO_CDP_CHANNEL_UNAVAILABLE`) ; `EngineHandle::request(EngineCommand) -> Pending`, `Pending::wait(Duration)` ; binaires `ego-lite` (headed), `ego-lite headless`, `ego-lite call <method> [json]`.

- [ ] **Step 1: Écrire les tests RPC** (`test_engine_pair()` et `connect(port, token)` sont des helpers du fichier)

```rust
#[tokio::test]
async fn rejects_missing_or_bad_token() {
    let (_engine, handle) = test_engine_pair();
    let info = serve("127.0.0.1:0".parse().unwrap(), handle).unwrap();
    assert!(connect(info.port, "wrong").await.unwrap_err().to_string().contains("401"));
    let mut ws = connect(info.port, &info.token).await.unwrap();
    ws.send(r#"{"jsonrpc":"2.0","id":1,"method":"ego.getBrowserVersion"}"#).await.unwrap();
    assert!(ws.next_json().await.unwrap()["result"]["version"].is_string());
}

#[tokio::test]
async fn closed_tab_fails_pending_within_two_seconds() {
    // createTab, closeTab, snapshot sur l'onglet fermé -> EGO_WEB_CONTENTS_UNAVAILABLE
}

#[tokio::test]
async fn user_control_blocks_mutations() {
    // createTaskSpace, claimTaskSpace, handOffTaskSpace, createTab -> EGO_TASK_SPACE_USER_IN_CONTROL
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-bridge --test rpc`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

`serve` : `tokio_tungstenite::accept_hdr_async`, `Authorization: Bearer`, 401 sinon. Chaque requête → `EngineCommand` + `Pending`, réponse sur la tâche tokio, timeout 30 s → `EGO_OPERATION_FAILED`. Les gardes de propriété sont appliquées **dans le thread moteur** (`Registry::require_agent_control`), jamais dans le dispatcher. `ego-lite headless` : `Engine` + boucle `pump()` 8 ms + pont + SIGINT. `ego-lite call` : client de test qui lit `bridge.json`.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-bridge --test rpc && cargo run -p ego-cli -- headless --help`
Expected: PASS + aide CLI.

- [ ] **Step 5: Committer**

```bash
git add ego/bridge ego/cli ego/engine && git commit -m "feat(bridge): local json-rpc bridge and headless cli"
```

---

### Task 8: Shell gpui et surface navigateur

**Files:**
- Create: `ego/ui/src/{view_model.rs,frame_loop.rs,app.rs,browser_view.rs}`, `ego/ui/tests/view_model.rs`
- Modify: `ego/ui/src/lib.rs`, `ego/cli/src/main.rs`

**Interfaces:**
- Consumes: `Engine`, `EngineEvent`, `gpui::{RenderImage, ImageSource, img, ObjectFit}`.
- Produces: `BrowserViewModel::{tabs, active_tab, spaces, banner}` ; `BrowserApp` ; `BrowserView` (point d'entrée pour komet) ; `fn frame_to_image(&image::RgbaImage) -> Arc<gpui::RenderImage>`.

- [ ] **Step 1: Écrire les tests du view model (sans fenêtre)**

```rust
#[test]
fn tab_strip_orders_tabs_and_marks_active() {
    let mut vm = BrowserViewModel::default();
    vm.apply(EngineEvent::TabCreated { tab: TabId(1), title: "A".into() });
    vm.apply(EngineEvent::TabCreated { tab: TabId(2), title: "B".into() });
    vm.select(TabId(2));
    assert_eq!(vm.active_tab(), Some(TabId(2)));
    assert_eq!(vm.tabs().len(), 2);
}

#[test]
fn handoff_banner_follows_ownership() {
    let mut vm = BrowserViewModel::default();
    vm.apply(EngineEvent::OwnershipChanged { space: SpaceId(1), ownership: Ownership::User });
    assert!(vm.banner().unwrap().contains("Take back control"));
    vm.apply(EngineEvent::OwnershipChanged { space: SpaceId(1), ownership: Ownership::Agent });
    assert!(vm.banner().is_none());
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-ui --test view_model`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

`app.rs` : rail des Spaces (nom + badge agent/user), bandeau *Take over* / *Hand back*, barre d'URL (Entrée → `navigate`), onglets (`Ctrl+T`, `Ctrl+W`, `Ctrl+L`), surface `img(frame_source).object_fit(ObjectFit::Contain)`. `frame_loop.rs` : tâche gpui toutes les 8 ms → `engine.pump()`, conversion `RgbaImage` → `RenderImage` (`smallvec![image::Frame::new(frame)]`, `Arc::new(RenderImage::new(..))`), puis `cx.notify()`. Les entrées gpui sont converties en `InputEvent` Servo sur le thread principal.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-ui --test view_model && cargo run -p ego-cli`
Expected: tests PASS ; fenêtre ouverte, `data:text/html,<h1>hello</h1>` visible, onglet créable/fermable.

- [ ] **Step 5: Committer**

```bash
git add ego/ui ego/cli && git commit -m "feat(ui): gpui shell with spaces, tabs and browser surface"
```

---

### Task 9: Compatibilité devtools/CDP (sous-ensemble)

**Files:**
- Modify: `components/servo/servo.rs` (exposer le port devtools au builder), `ego/bridge/src/dispatch.rs`
- Create: `ego/bridge/tests/cdp.rs`

**Interfaces:**
- Produces: `EngineConfig.devtools_port: Option<u16>` ; `ego.sendCDPMessage({tab, message})` route les méthodes `Runtime.evaluate`, `Page.navigate`, `Page.getNavigationHistory` vers `components/devtools` ; toute autre méthode renvoie `EGO_CDP_SEND_FAILED`.

- [ ] **Step 1: Écrire le test**

```rust
#[tokio::test]
async fn runtime_evaluate_is_forwarded_to_devtools() {
    let r = call("ego.sendCDPMessage", json!({"tab": 1, "message": {"id": 1, "method": "Runtime.evaluate", "params": {"expression": "1+1"}}})).await;
    assert_eq!(r["result"]["result"]["value"], 2);
}

#[tokio::test]
async fn unsupported_method_reports_cdp_send_failed() {
    assert_eq!(err_code("Page.captureScreenshot").await, "EGO_CDP_SEND_FAILED");
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-bridge --test cdp`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

Activer `devtools_server_enabled` quand `devtools_port` est fourni, se connecter en client WebSocket au serveur local, traduire l'appel, renvoyer le JSON brut. Ne pas prétendre au CDP complet : documenter les trois méthodes supportées dans le README.

- [ ] **Step 4: Vérifier**

Run: `cargo test -p ego-bridge --test cdp`
Expected: PASS.

- [ ] **Step 5: Committer**

```bash
git add components/servo ego/bridge && git commit -m "feat(bridge): forward a cdp subset to servo devtools"
```

---

### Task 10: Intégration komet, packaging Linux, documentation

**Files:**
- Create: `docs/komet-integration.md`, `scripts/package-linux.sh`, `ego/ego-lite.desktop`
- Modify: `README.md`, `ego/ui/src/lib.rs`, `ego/cli/src/main.rs`

**Interfaces:**
- Produces: feature `komet` (off par défaut) réexportant `BrowserView` sans `BrowserApp` ; `scripts/package-linux.sh` ; guide d'intégration.

- [ ] **Step 1: Écrire le test de compilation**

```rust
// ego/ui/tests/embed.rs
#[test]
fn browser_view_is_constructible_without_the_shell() {
    let _ctor: fn(&mut gpui::App) -> BrowserView = BrowserView::new;
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test -p ego-ui --features komet --test embed`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

`BrowserView::new(cx)` rend la surface + le bandeau ; `BrowserApp` l'enveloppe. `docs/komet-integration.md` : komet ajoute `ego-ui` en dépendance git (révision du fork), ajoute un type d'onglet `Browser` dans `crates/ui/src/shell`, réutilise son `gpui` (même rev), désactive le pont réseau en mode intégré, garde le `data_dir` de komet. `scripts/package-linux.sh` : `cargo build --release -p ego-cli`, installe binaire + `.desktop`, liste les dépendances système (mesa, harfbuzz, cmake, python3-mako).

- [ ] **Step 4: Vérifier l'ensemble**

Run: `cargo fmt --all --check && cargo clippy --workspace -- -D warnings && cargo test -p ego-core -p ego-engine -p ego-bridge -p ego-ui`
Expected: PASS.

- [ ] **Step 5: Committer**

```bash
git add -A && git commit -m "feat: embeddable browser view, linux packaging and komet docs"
```

---

## Auto-revue

- **Couverture** : Spaces/ownership → tâche 2 ; snapshot multi-frames (y compris iframes cross-origin) → tâche 3 ; API `ego.*` → tâches 3, 5, 7 ; persistance profil/logins → tâche 6 ; shell quotidien minimal → tâche 8 ; CDP minimal → tâche 9 ; intégration komet → tâche 10. Hors v1 et documenté dans l'ADR : extensions, téléchargements, favoris, migration Chrome, sync, `animationHighlightMouseToPosition` (`EGO_OPERATION_FAILED`).
- **Cohérence des types** : `SpaceId`/`TabId`/`ProfileId` définis tâche 2 et utilisés partout ; `Snapshot`/`RefEntry.reference: String` définis tâche 3 et consommés tâches 5 et 7 ; `EngineHandle::request` défini tâche 7, utilisé uniquement par le pont ; `EngineConfig` étendu en tâche 9 sans casser les appels existants.
- **Review Focus** : points 1–2 → tâche 3 ; point 3 → tâches 2 et 7 ; point 4 → tâche 7 ; point 5 → tâche 4 ; point 6 → tâche 7 ; point 7 → tâche 6.

## Risques connus

- **Compatibilité web de Servo** : inférieure à Chromium (captchas, grosses SPA). Le fork permet de corriger le moteur au cas par cas, mais pas de garantir Google/Cloudflare.
- **Injection dans les frames** : si `UserScript` n'atteint pas les iframes cross-origin dans Servo, la tâche 3 doit escalader vers un patch `components/script` (coût non trivial). C'est le point de sortie n°1 du plan.
- **Rendu logiciel** : copie CPU d'une frame par onglet actif (`read_to_image`). 30 fps visés ; optimiser (texture partagée) si l'usage quotidien souffre.
- **gpui sous toolchain Servo 1.99.1** : vérifié dès la tâche 1 ; repli workspace séparé documenté.
- **Premier build** : 15–25 min, exige cmake, python3-mako, libharfbuzz-dev, pkg-config.
