# ADR 0001 — Architecture du navigateur `webview` (fork Servo, stack komet)

Date : 2026-10-30
Base : `jomvick/servo` `4dfd2701b` (branche `webview`), 5 PR upstream devant le checkout `0626de7b4`
Statut : decisions D1-D4, D6-D8, D10 et D11 prises sur preuves de code, D3 **patché et vérifié crate par crate** ; **D5 et D9 prises sur mesure** (sondes `components/servo/tests/phase0_probe.rs`, résultats ci-dessous) ; reste ouvert : la vérification empirique de D7 depuis un workspace externe

## Contexte

Objectif : un navigateur qu'on lance seul **et** une brique réutilisable dans komet, avec un pilotage par agent (snapshot de page, actions, spaces) dans les deux cas. Le moteur reste Servo ; ce qui est reconstruit, c'est la couche shell/UI, dans la stack de komet (gpui épinglé à la revision Zed `e2ddcc6`, tokio, serde, edition 2024).

## Décisions

### D1 — Fork Servo plutôt que crates publiées
Le dépôt de travail est un fork de `servo/servo`, et le moteur est consommé par chemin (`components/servo`).
Pourquoi : le produit (pilotage agent, snapshot multi-frames) exigera probablement des corrections dans le moteur, et un fork permet de les faire au cas par cas tout en restant rebaseable. Conséquence : `upstream` = `servo/servo`, `origin` = le fork, et la règle D2.

### D2 — `components/**` et `third_party/**` gelés en v1
Aucune modification du moteur, à la seule exception du patch D3 et du fichier de test additif `components/servo/tests/phase0_probe.rs`.
Pourquoi : chaque ligne touchée dans le moteur est une ligne à rebaser à chaque synchro upstream. Les sondes vivent dans l'arborescence du moteur uniquement parce qu'elles réutilisent son harnais d'intégration (`components/servo/tests/common/mod.rs`).
État : l'exception est consommée, exactement deux fois. `git diff --stat` = `components/allocator/{Cargo.toml,lib.rs}` + `ports/servoshell/Cargo.toml`, 17 insertions, et un fichier de test non suivi. Rien d'autre n'est touché, et c'est ce qui rend le rebase indolore.

### D3 — Allocateur global : feature-gate côté Servo, mimalloc côté komet
`components/allocator/lib.rs:11-16` enregistre un `#[global_allocator]` inconditionnellement, et ce crate est dans le graphe de `fonts` (`Cargo.toml:82`), `paint` (`:41`), `malloc_size_of` (`:39`), `profile` (`:21`) : dépendre du moteur suffit à l'enregistrer. komet déclare déjà mimalloc (`komet/apps/komet/src/main.rs:147`), choisi pour son retour de pages au système (`komet/docs/memory-plan.md`). Deux enregistrements dans un même binaire = erreur de compilation.
Décision : nouveau feature **opt-in** sur `servo-allocator` qui gate les deux statics ; `ports/servoshell` l'active (il l'active déjà pour `use-jemalloc`, `Cargo.toml:50,111`), l'intégration komet ne l'active jamais.
Coût assumé : sous mimalloc, `usable_size` et les compteurs `mallctl` (`allocator/lib.rs:143,73`) ne rendent plus les mesures mémoire fines de Servo ; le `--mem-report` devient approximatif. Patch volontairement remontable en PR upstream (même problème pour tout embeddeur de `ffi/capi`).

**Patch appliqué** (`docs/phase0/0001-allocator-global-allocator.patch`, 3 fichiers, 17 insertions) et vérifié sur les deux moitiés du contrat. Côté `servo-allocator` seul, combinaison par combinaison (`cargo check`) : sans feature ✓ (chemin komet), `global-allocator` ✓ (chemin servoshell, strictement équivalent à avant), `global-allocator + allocation-tracking` ✓, `allocation-tracking` seul → le `compile_error!` refuse la combinaison, comme prévu. Côté moteur : le **chemin hôte** (feature éteinte) est le plus risqué — c'est celui où `crate::ALLOC` n'existe plus — et il est validé en grand, `cargo test -p servo --test phase0_probe` a reconstruit `servo-allocator` sans le feature, puis `fonts`/`paint`/`malloc_size_of`/`profile` par-dessus, et les deux sondes produisent les mêmes résultats qu'avant le patch. Le **chemin servoshell** (feature allumée) est validé sans reconstruction par la résolution du graphe : `cargo tree -p servoshell -e features --no-default-features --features default_base -i servo-allocator` montre `servo-allocator feature "global-allocator"` sous `servoshell feature "default_base"`, donc le nom du feature et sa cible dans le manifeste sont corrects.

Ce que la relecture du graphe ajoute à la décision : le seul endroit du moteur qui traverse l'allocateur du processus est `components/fonts/platform/freetype/library_handle.rs:15`, qui prend `malloc`/`free`/`realloc` **ensemble** depuis `servo_allocator::libc_compat` pour les callbacks mémoire de FreeType. Le triangle est clos sur lui-même — aucune de ces pointures n'atteint `std::alloc` — donc il n'y a pas de mésalliance allocateur/libc à corriger quand `global-allocator` est éteint. Ça tient aussi parce que komet n'active pas `use-jemalloc` : sans lui, `libc_compat` ré-exporte `libc::{malloc, free, realloc}` (`allocator/lib.rs:194`, le module `platform` sans jemalloc commence `:165`) et non les symboles jemalloc (`:160`). L'invariant à garder : **ne jamais activer `use-jemalloc` sans `global-allocator`**, sinon le moteur allocatorait en jemalloc derrière le `#[global_allocator]` de l'hôte.

Règle de build qui en découle, **mesurée le 2026-10-31** : le gate est choisi par **l'invocation**, pas par le crate. `cargo tree -e features -i servo-allocator -p webview-engine` ne montre **aucune** occurrence de `global-allocator` ; la même commande avec `-p servoshell --no-default-features --features servoshell/default_base` en plus montre `servo-allocator feature "global-allocator"`, donc les deux sélections s'unifient dans la même invocation et rallument la feature pour tout le graphe, `webview-engine` compris. Ce n'est pas le cas de komet, qui est un workspace séparé — le cas réel est donc le premier, et la conséquence pratique est : **ne jamais costruire `webview-engine` et `servoshell` dans un même `cargo build`/`cargo test`/`cargo clippy`**, sinon le moteur part avec l'allocateur global du fork et se bat contre le mimalloc de komet au link. Le test `webview-engine` tourne seul (`cargo test -p webview-engine`), c'est suffisant.

### D4 — Un thread dédié au moteur, pas le thread principal de gpui
Servo 0.7 est `Rc`/`RefCell`, `!Send` (`Servo`, `WebView`, delegates), et gpui impose son `App`/`Window` sur le thread principal.
Décision : un thread unique porte `Servo` + `Registry` + le `RenderingContext` ; seul un `Arc<RgbaImage>` de frame traverse vers gpui, et le `EventLoopWaker` (`components/shared/embedder/lib.rs:241`) poste sur le scheduler gpui.
Pourquoi : `WebView::paint()` + `read_to_image()` sont du travail lourd et synchrone ; sur le thread principal, une grande page gèlerait la fenêtre de komet entière. Cela remplace la décision D7 du plan ego-lite (« un seul thread pour Servo, le Registry et gpui »), qui ne tenait pas ce risque.
Contrainte mesurée (échec de la sonde 2, `/tmp/servo-probes.log`) : `Opts` est un singleton de processus (`components/config/opts.rs:279`, `OnceLock::set` → panique « Already initialized »). Donc **un moteur par processus**, pas « un par thread » ni « un par onglet » : les onglets et les spaces sont des `WebView` du même `Servo`. Ça ferme une question de design pour `webview-engine` (pas de multi-instances à prévoir) et explique pourquoi les deux sondes doivent tourner dans deux processus séparés.

### D5 — Chemin des pixels : sortie CPU d'abord — **mesuré, voie CPU retenue**
Le moteur ne rend qu'en OpenGL/WebRender via surfman (`components/shared/paint/rendering_context.rs:34`) ; gpui rend en Metal (macOS), wgpu (Linux), D3D11 (Windows). Aucune interop GPU partagée, sauf `Window::paint_surface(CVPixelBuffer)` côté macOS seulement (`gpui/src/window.rs:4544`).
Voie v1 retenue : `RenderingContext::read_to_image` (`:45`) → bitmap → texture gpui, en pilotage à événement (`notify_new_frame_ready` → redraw → `paint()` + `present()`, modèle de `components/servo/examples/winit_minimal.rs:44-131`), **pas** le polling 8 ms du plan ego-lite.
Faits déjà établis : le harnais de test amont utilise `SoftwareRenderingContext` (`tests/common/mod.rs:13,37`), donc la sortie CPU est un chemin officiel ; surfman 0.14 n'a plus de dépendance `osmesa` (absent du `Cargo.lock` et des listes `python/servo/platform/linux_packages/{apt,dnf}`) → repose sur llvmpipe. **Confirmé sur la machine cible** : `servoshell -z -x -o /tmp/servo-shot.png --window-size=1280x800` (headless, aucune session graphique, donc pas de `DISPLAY`) rend la page attendue. Coût mesuré dans ces conditions — build debug, machine déjà occupée par un `cargo build -j2` : **13,8 s** de bout en bout (démarrage + init moteur + une frame + PNG), **pic RSS 207 Mo** pour une page triviale. Le RSS est le chiffre qui intéresse D1 (plancher mémoire d'un moteur in-process dans komet) ; le temps n'est pas comparable, la sonde 1 le redonne à chaud et sans concurrent.
Les deux mesures à inscrire sont inscrites. Sonde 1 (`components/servo/tests/phase0_probe.rs::probe_frame_path_and_cost`), profil dev, llvmpipe sans GPU ni `DISPLAY`, 4 cœurs, trois essais de 20 frames par taille, un processus par test. Fourchette sur **trois** campagnes, la troisième exécutée après le patch D3 avec le moteur reconstruit et un `cargo build --bin komet` concurrent (mémoire libre ~1 Go) :

| taille | `paint()` frame sale | `paint()` sans frame sale (plancher) | `read_to_image` | les deux |
| --- | --- | --- | --- | --- |
| 500×500 | 0,55 – 1,42 ms | 0,57 – 0,93 ms | 4,07 – 6,46 ms | 4,7 – 7,9 ms (127 – 213 img/s) |
| 1280×800 | 0,65 – 1,04 ms | 0,70 – 1,45 ms | 6,93 – 10,27 ms | 7,76 – 11,15 ms (90 – 129 img/s) |

La dispersion vient du concurrent, pas de la méthode : sans lui les trois essais d'une même campagne tenaient à ±5 %. Le plancher (`paint()` sans rien de sale) reste dans tous les cas au niveau du `paint()` sale, ce qui est le résultat, pas du bruit.

Trois choses s'en déduisent, aucune n'allait :

1. **L'ordre des octets est RGBA** — `[255, 0, 0, 255]` relevé en (60, 60) sur un carré `%23f00`, et blanc hors du carré. Côté gpui c'était tranché par le code : `RenderImage` est documenté « in BGRA format » (`gpui/src/assets.rs:45-47`) et son décodage fait `pixel.swap(0, 2)` (`gpui/src/platform.rs:2467-2470`). Le `swap(0, 2)` par pixel sur la frame entière n'est donc pas une option, c'est le coût d'entrée de la voie CPU (non mesuré ici, 1 Mpx = 4 Mo à parcourir).
2. **Ce qui sort des pixels n'est pas le raster.** `read_to_image` fait ~90 % du coût par frame et ne suit pas la surface (≈ 4 ms fixes + ≈ 260 Mpx/s) ; `paint()` une frame sale coûte le même temps qu'une frame propre — la bande de 20 px redéfilée disparaît dans le `make_current` + clear complet + appel WebRender. Le « travail lourd et synchrone » de D4 est à attacher à `read_to_image`, pas à `paint()`.
3. **Le plafond d'une vue pilotée par les événements est la latence d'entrée, pas le rendu.** Un défilement à la molette met 11 – 16 ms à aller du thread peinture au thread script et revenir — le delta n'est appliqué qu'au retour de `notify_input_event_handled` (`components/paint/webview_renderer.rs:1201`) — et ce temps ne dépend pas de la taille de l'image. À 1280×800, une frame fraîche répond donc à un événement ~20 ms plus tôt : ~50 images/s de bout en bout, mesuré, sans que le thread moteur ait fait autre chose que `paint()` + `read()`.

Conséquence : dans le **pire** profil mesuré (debug + llvmpipe + moteur reconstruit + un `cargo build` concurrent sur 4 cœurs avec ~1 Go de mémoire libre), la sortie CPU donne 90 – 129 img/s à 1280×800, soit 8 – 11 ms du seul thread moteur par frame (D4) et rien sur le thread principal de gpui. Un flux d'aperçus à 30/s occupe le thread moteur 25 – 35 % du temps dans ces conditions dégradées, et beaucoup moins quand la machine est libre : marge d'un facteur ~3 même au pire. Un build `--release` ne changerait pas la conclusion, il l'accentuerait. **D5 tranché : `read_to_image` → bitmap → texture gpui, aucune interop GPU.**

Deux pièges d'implémentation, tombés dessus en mesurant, à écrire dans `webview-engine` :
- **ne pas peindre dans `notify_new_frame_ready`** : le delegate du harnais amont le fait (`components/servo/tests/common/mod.rs:135`) et consomme la frame sale que l'embeddeur voulait rendre. Et le signal est un *niveau*, pas un front — `send_new_frame_ready_messages` (`components/servo/servo.rs:426-434`) rejoue tant que `needs_repaint` est posé : **276 à 299 notifications** (284 à la troisième campagne) alors que le chargement de cette page `data:` de deux `<div>` ne change l'image visible qu'une fois. L'ordre de grandeur est celui du refresh driver de repli, 120 Hz en l'absence de vsync (`components/paint/refresh_driver.rs:45`) ; le compte rendu, pas l'attribution. Un embeddeur qui peint à chaque notification peint une page immobile plus de deux cents fois par chargement ; c'est le redraw de gpui qui doit dicter la cadence.
- **`LoadStatus::Complete` ne contient pas le contenu** : le premier `paint()` d'une page `data:` rend un blanc, le carré rouge n'apparaît qu'à une scène ultérieure. Un snapshot déclenché sur la première `notify_new_frame_ready` renverrait une image vide. La sonde attend une accalmie de 20 ms sans scène nouvelle avant de peindre ; un shell qui se contente de la première image peinte peut montrer une page vide.
Voie d'upload validée sans patcher gpui : `RenderImage::new()` prend `impl Into<SmallVec<[image::Frame; 1]>>` (`gpui/src/assets.rs:60`), donc un raster brut suffit — pas d'encodage PNG par frame.
Contrainte connue de l'upload : `RenderImage::new()` consomme un `ImageId` incrémenté à chaque appel (`gpui/src/assets.rs:61-68`) → une frame = une entrée de sprite atlas, donc RSS monotone, la pathologie déjà documentée chez komet, corrigée chez eux par `Img::evict()` (`gpui/src/elements/img.rs:609`). Deux issues à trancher avec les chiffres : évincer à chaque frame, ou ajouter au fork gpui une « surface externe » persistante.

### D6 — Couches réutilisables, shell autonome facultatif

Six crates vivent sous `webview/` : `webview-core` (modèle pur), `webview-bridge`
(contrat et scripts), `webview-engine` (adaptateur Servo), `webview-runtime`
(thread moteur et client `Send`), `webview-ui` (shell GPUI) et `ego-lite`
(exécutable et serveur local). Aucune ne dépend d'une crate `komet-*`.
Komet peut intégrer le runtime dans sa propre vue sans lancer une deuxième
application GPUI ; le binaire autonome réutilise exactement les mêmes couches.
Le shell épingle les révisions GPUI de Komet ; le runtime ne dépend pas de GPUI.
Les noms publics `ego.*` restent compatibles avec le contrat existant.

Garde-fou du modèle, écrit dans `webview-core` : la propriété (`Ownership`) ne borne que **les mutations qui donnent du travail à l'agent** — `create_tab` exige `Agent`. Les suppressions (`close_tab`, `close`) ne sont pas gardées, parce que l'utilisateur doit pouvoir fermer un onglet d'un space qu'il tient et l'agent refermer un space qu'il a complété. `Ownership::Done` est terminal pour les quatre opérations de main (`claim`/`hand_off`/`take_over`/`complete` → `EGO_TASK_SPACE_INACTIVE`) mais pas pour `close`. Conséquence mesurable dans les tests : `close` rend les `TabId` supprimés, c'est par là que le moteur libère les `WebView`.

### D7 — Configuration de build à dupliquer dans komet : presque rien
`browser/.cargo/config.toml` pose un `[env]` que je croyais contraignant ; il ne l'est pas, vérification faite (Cargo lit `[env]` depuis le répertoire d'appel, jamais depuis une dépendance — donc ce qui n'est pas repris doit être **inutile**, pas « toléré ») :

- `RUSTC_BOOTSTRAP = "crown,script,script_webcrypto,script_webgpu,script_bindings,style_tests,mozjs,mozjs_sys"` : c'est une **liste blanche de crates**, pas un activateur global, et le toolchain Servo est **stable 1.99.1** (`rustc --version`), pas nightly. Chaque crate listé ne met son `feature(...)` nightly que sous condition crown — `components/script/lib.rs:5` et `components/script_bindings/lib.rs:5` : `#![cfg_attr(crown, feature(register_tool))]`, idem `mozjs-0.26.13/src/lib.rs:13` (`feature = "crown"`). O et crown est **hors** d'un build normal : la ligne rustc observée pour `script_bindings` passe `--check-cfg cfg(crown)` mais pas `--cfg crown`. Le `[env]` existe pour le mode `--crown` (régénération des bindings), pas pour le build courant. Aucun `-Z` dans `[build]` (seul `rustdocflags` y figure).
- `SERVO_STYLE_THREAD_STACK_SIZE_KB = "8192"` : **aucun code ne le lit dans l'arborescence** (`grep -rni THREAD_STACK_SIZE`, hors `target/` et hors le clone dupliqué) — ni code Servo ni dépendance du registry. Le PR #43888 cité en commentaire ne correspond plus à rien d'actif ici.
- `MACOSX_DEPLOYMENT_TARGET`, linkers android/windows, `CC_*_PC_WINDOWS_MSVC` : hors périmètre Linux x86_64 de v1 (D2).

Donc : **rien à dupliquer pour un build Linux sans crown**, et surtout ne pas recopier `RUSTC_BOOTSTRAP` dans komet — ce serait autoriser nightly sur tout le graphe de komet pour un bénéfice nul. La correction à mon propre état précédent : j'avais écrit que `components/**` ne contient aucun `#![feature(...)]` ; c'était une erreur de motif de recherche (`^#!\[feature(` ne voyait pas la forme `cfg_attr`). Ce qui est vrai, c'est qu'aucun n'est **actif hors crown**.

Reste une vérification empirique, une fois le patch allocateur posé : builder `servo` depuis un workspace externe, toolchain stable, sans aucun `[env]` repris — si ça passe, D7 se ferme sur « rien à dupliquer » ; si ça casse, la casse nomme précisément la clé manquante.

### D8 — Diet de features v1
`multiprocess` **activé** (le contenu tourne hors process, donc le JS ne bloque pas le thread moteur de D4), `devtools` activé (sous-ensemble CDP), **GStreamer désactivé** en v1 → pas de `<video>`/audio. `webgl` garde ANGLE, poste de build incompressible. Validé indirectement : `servoshell` distingue déjà `default_base` (sans média) de `default` (`ports/servoshell/Cargo.toml`).

### D9 — Périmètre agent repris du plan ego-lite
Spaces + ownership (`claim`/`handOff`/`takeOver`/`complete`), snapshot texte avec réfs et budget de temps, actions par réf avec repli sur `InputEvent` quand `event.isTrusted` est exigé, pont JSON-RPC sur `127.0.0.1` + Bearer, persistance via `Opts::config_dir` (Servo écrit déjà `cookie_jar.json` et `webstorage.sqlite`, `net/resource_thread.rs` et `storage/webstorage/engines/sqlite.rs`).
Question ouverte n°1, tranchée par la sonde 2 (`probe_userscript_subframe_reach`) : quatre frames, chacun se signalant lui-même par `NATURAL|` — c'est ce qui distingue « le frame n'a pas chargé » de « le frame est chargé mais l'injection ne l'atteint pas ». Les quatre chargent ; l'injection, non :

| frame | chargé | `UserScript` | atteint depuis le top (`contentWindow.eval`) |
| --- | --- | --- | --- |
| principal | oui | oui | oui (au `load`, 3 sous-frames) |
| `<iframe src="child.html">`, même origine | oui | **non** | oui |
| `<iframe srcdoc>` (`about:srcdoc`) | oui | **non** | oui |
| `<iframe src>`, origine étrangère (port distinct) | oui | oui | **non** (`SecurityError`) |

Mon indice statique était faux dans les deux sens : l'injection ne suit pas « tout frame document » (`components/script/dom/userscripts.rs:11-19`) — elle manque les deux frames où elle devrait être la plus simple — et elle réussit sur le seul cross-origin, précisément le cas où l'`UserContentManagerId` devait traverser un thread script distinct. La cause du manque n'est pas établie ; ce qui l'est, et suffit à décider, c'est la **complémentarité exacte** des deux mécanismes, mesurée l'un contre l'autre sur les mêmes frames.

Décision : **l'injection agent est double**. Un `UserScript` (principal + cross-origin) **et** un parcours `document.querySelectorAll('iframe')` → `contentWindow.eval` depuis le monde du frame principal (même origine + `srcdoc`). Aucun des deux seul ne couvre les quatre. Le second ne peut pas être un `UserScript` — il doit tourner dans le monde du frame principal, donc partir de `WebView::evaluate_javascript` (`components/servo/webview.rs:741`) après le `load` ; son blocage sur le cross-origin est la politique d'origine, pas un défaut à contourner.

Deux effets de bord de la même sonde, à traiter dans `webview-engine` : le `load` du frame principal se déclenche **deux fois** (`TOP_CHARGE|0` sur le document initial, puis `TOP_CHARGE|3`), donc un injecteur branché dessus doit être idempotent ; et un `console.log` émis depuis le monde d'un sous-frame remonte bien au delegate du `WebView` parent (`components/servo/servo.rs:790-795`) — c'est le canal par lequel un snapshot multi-frames revient, et il fonctionne pour les quatre frames.

**Vérifié dans le chemin d'intégration** (2026-11-01, `webview/engine/tests/preload.rs`, donc avec `UserContentManager` + `WebView::evaluate_javascript` et plus le harnais amont de Servo). Quatre frames : le top servi en http, un `srcdoc`, une iframe `data:`, une iframe http d'origine étrangère. Le `UserScript` est installé par `Engine::install_preload` avant la création de l'onglet, et chaque enfant se signale lui-même par une requête HTTP (`/report?userscript=`) parce que le cross-origin est illisible depuis le top par construction. Résultats, ligne `MESURE-PRELOAD` du test :

| frame | `UserScript` | lecture depuis le top |
| --- | --- | --- |
| principal (http) | oui | oui |
| `<iframe srcdoc>` | **non** (`preload: false`) | oui — le `title` se lit |
| `<iframe src="data:text/html,…">` | oui | **non** (`SecurityError`, `contentDocument` à `null`) |
| `<iframe src>` http étranger | **oui** (le rapport HTTP porte `userscript=true`) | **non** (`SecurityError`) |

C'est exactement la complémentarité mesurée par la sonde 2, reproduite ici avec les types de l'adaptateur : l'injection double est retenue, pas une hypothèse qui restait à confirmer. Deux points de plus, qui ne se déduisaient pas de la table précédente : `postMessage` enfant → top passe pour les **deux** enfants cross-origin (le top reçoit `ego-hello` des deux origines), donc l'agrégation du snapshot multi-frames (D2) a un canal légal ; et `evaluate` sur un `throw` renvoie l'exception à l'appelant plutôt que de vider le callback — le message contient bien le `x` levé.

### D10 — Le format du snapshot est fixé par le parseur, pas par le JS
`webview-core` exporte `SnapshotOptions`/`RefEntry`/`Snapshot`/`parse_snapshot` (tâche 3 du plan, moitié pure faite ; le walker JS vient avec `webview-bridge`). Le contrat d'entrée est donc écrit **avant** le producteur, et le JS devra le respecter à la lettre :

```jsonc
{ "title": "Order", "url": "https://…",
  "frames": [ { "path": "f1", "depth": 1,
                "nodes": [ { "role": "button", "name": "Pay now", "ref": "e3",
                             "value": "…", "unreadable": true, "children": [] } ],
                "timeout": true } ] }
```

Quatre décisions de rendu qui ne sont pas des détails :
- **`reference` est composée côté parseur** (`{path}:{ref}`) : le JS numérote les éléments par frame, et deux frames peuvent toutes deux avoir un `e1`. Une réf sans path serait inadressable pour `click`. Un frame **sans `path`** est une erreur `EGO_SNAPSHOT_FAILED`, pas une réf `:e1` silencieuse.
- **Le budget ne mange pas l'en-tête ni les réfs.** `max_chars` borne le corps ; `# {titre}\n{url}` est toujours entier, et `Snapshot::refs` est complet même quand le texte est coupé — sinon un agent tronqué ne peut plus cliquer. Une coupe est marquée par une dernière ligne `…`, parce qu'un agent qui lit la fin croirait à une fin de document.
- **Une frame absente du résultat n'est jamais silencieuse** : `timeout: true` devient une ligne `[frame timeout] f1`, et un iframe dont le contenu n'est pas lisible devient `- iframe "…" [unreadable]`. C'est le point 1 du Review Focus du plan, et le point 2 (une iframe qui ne répond pas doit rester dans le budget de 1,5 s) est traité par le JS, pas ici.
- **Une ligne par nœud, format `- {role} "{name}"[ [ref=…]][ value="…"]`, deux espaces par niveau** (profondeur de frame + profondeur dans le document). Le texte de page est assaini : blancheurs réduites à une espace, guillemets doubles remplacés — sinon le contenu de la page casse le format qui est censé la décrire.

### D11 — Le pont parle JSON-RPC 2.0 et rend les codes `EGO_*` dans la plage serveur
`webview-bridge` est écrit avant le moteur, parce que son contrat ne dépend pas de lui : l'enveloppe JSON-RPC 2.0, les vingt et un noms `ego.*` de la v1 du plan, et la mapping `EgoError` → réponse d'erreur. Ce que le code fixe, et qui ne se déduit pas de la spécification JSON-RPC :

- **Un appel sans le préfixe `ego.` n'existe pas.** `snapshot` seul est un `METHOD_NOT_FOUND` dont le message cite le nom reçu — un agent qui se trompe de préfixe doit le lire, pas croire la méthode absente.
- **Une erreur du modèle reste une erreur JSON-RPC bien formée** : `error.code` = `-32000` (plage serveur réservée par la spec, qui exige un nombre), `error.message` = la chaîne `EGO_TASK_SPACE_USER_IN_CONTROL`, `error.data` = `{code, message}` entiers. Le double emploi est voulu : les clients d'ego matchent sur la chaîne, les clients JSON-RPC sur le nombre, et aucun des deux n'a à connaître l'autre.
- **Les défauts viennent du modèle, pas du protocole.** `snapshot` sans `maxChars` ni `interactiveOnly` reçoit `SnapshotOptions::default()` de `webview-core` : le budget de 12 000 caractères n'existe qu'à un seul endroit.
- **`ref` et `reference` sont deux orthographes du même champ** (ego écrit `ref`, le modèle nomme `reference`) ; `reference` gagne si les deux sont présents.
- **Une notification (pas d'`id`) s'exécute et ne reçoit pas de réponse** ; une enveloppe qui n'est pas du JSON répond avec `id: null`, et une enveloppe mal formée *dont l'`id` est lisible* y répond avec son `id` — sinon l'appelant ne peut rien raccorder.
- `useTaskSpace` (nom public d'ego) **n'a pas d'équivalent en v1** : chez ego il désigne le space courant de l'agent, ce que le modèle de D6 fait déjà avec `claimTaskSpace`. C'est une divergence assumée avec la liste des noms, pas un oubli.

Le transport autonome est HTTP POST JSON-RPC sur loopback, avec un token
privé (`Authorization: Bearer`), validation de `Host` et rejet des requêtes
portant `Origin`. Les notifications ne produisent pas de réponse JSON-RPC.
Le runtime utilise une file bornée à 64 commandes et une échéance de 30 s ;
une évaluation JavaScript conserve sa propre limite de 10 s. Les commandes
expirées ne sont pas démarrées. Le dispatcher applique `require_agent_control`
dans le thread moteur, pas dans le pont. Les commandes `ui.*` ne sont accessibles
qu'en processus ; le serveur réseau n'expose que `ego.*`.


## Mesure de référence (Phase 0)
Build de `servoshell` sur le fork, rev `4dfd2701b`, profil dev, `RUSTC_BOOTSTRAP` du workspace, `CARGO_BUILD_JOBS=2`, `DEBUG=0`, toolchain épinglée `1.99.1` :

- `--no-default-features --features default_base` (donc sans GStreamer) : **38 min 47 s**, binaire `target/debug/servoshell` **508 Mo**, `target/` **9,0 Go**, `./servoshell --version` → `Servo 0.7.0-4dfd2701b`.
- Avec `--default-features` : échec sur `gstreamer-sys` tant que les paquets système ne sont pas installés (D8), donc non mesuré.
- Disque : 20 Go libres sur `/home` après ce build, soit une marge d'un seul build complet. `cargo test` sur un crate du workspace recharge `servo` avec `testbinding`+`tracing` (features différentes) → reconstruction du crate et de ses dépendanceurs ; c'est la raison pour laquelle les mesures des sondes sont groupées dans un seul fichier de test.

Le pic RSS d'un moteur en marche n'est pas une donnée de build : il est mesuré sous `/usr/bin/time -v` par `docs/phase0/run-probes.sh`. Chiffre retenu pour D1, un `Servo` complet avec contexte WebRender logiciel, une page chargée et une `WebView` : **188 – 198 Mo** de pic RSS par processus de test, stable sur les trois campagnes et les deux sondes, et 197 – 207 Mo pour un `servoshell` headless à 1280×800. C'est le plancher mémoire d'un moteur in-process dans komet, avant les pages réellement ouvertes.

Le fait que ce chiffre sorte d'un **processus de test** et non d'une page n'est pas un détail de mesure : `Opts` est un singleton de processus, donc deux moteurs dans le même binaire sont impossibles (D4). Le script lance délibérément chaque test dans son propre processus pour que le RSS relevé soit celui d'un moteur, pas de deux.

## État d'avancement

Il n'est pas dans ce document : un ADR consigne des décisions, pas un avancement. Voir [`../../webview/README.md`](../../webview/README.md) pour le fonctionnement actuel et [`../phase0/README.md`](../phase0/README.md) pour les mesures historiques. Les mesures de D5 et de D9 y sont rappelées avec leur commande de reprise (`./docs/phase0/run-probes.sh`, ~30 s de rebuild, ~10 s de mesures).

## Conséquences sur le planning

- La machine de développement (7 GB RAM, 4 cœurs, 31 GB disque) ne peut pas héberger le CI complet du moteur : le pipeline « moteur » doit produire des artefacts, le pipeline « shell/komet » les consommer.
- 14 paquets système de la liste `dnf_base.txt` étaient absents ; le build de référence complet (`--default`) échoue sur `gstreamer-sys` tant que GStreamer n'est pas installé, ce qui recoupe D8.
- La compatibilité web de Servo (captchas, grosses SPA) est un risque produit, pas technique : le pilotage agent sur des sites réels peut imposer un moteur de repli externe.
