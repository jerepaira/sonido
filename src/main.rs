//! Sonido — reproductor de música nativo para Linux basado en addons.
//!
//! Arranque por defecto: GUI. Con subcomandos, modo consola (útil para
//! verificar un addon sin abrir la ventana).

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "sonido",
    about = "Reproductor de música nativo para Linux que consume addons (protocolo Eclipse Music)",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Verbosidad del log (RUST_LOG tiene prioridad). Ej: -v, -vv, --log trace
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Ancho inicial de la ventana
    #[arg(long, default_value_t = 1360)]
    width: u32,

    /// Alto inicial de la ventana
    #[arg(long, default_value_t = 860)]
    height: u32,

    /// Fuerza el backend de audio 100% Rust aunque exista libmpv
    #[arg(long)]
    no_mpv: bool,

    /// Renderer: wgpu (Vulkan/Metal) o glow (OpenGL). Por defecto wgpu.
    #[arg(long, default_value = "wgpu")]
    renderer: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// Abre la interfaz gráfica (comportamiento por defecto)
    Gui,

    /// Instala un addon desde su URL de manifest.json
    Add {
        /// URL del manifest.json o la base del addon
        url: String,
    },

    /// Lista los addons instalados
    List,

    /// Desinstala un addon por id o por URL
    Remove { id_or_url: String },

    /// Cambia un setting declarado por el addon
    Set {
        /// id del addon (p.ej. com.yourname.tidal.hifi)
        addon: String,
        /// clave del setting (p.ej. quality)
        key: String,
        /// valor (p.ej. MAX). Cadena vacía = volver al default del addon
        value: String,
        /// guardar el valor para red celular (settings perNetwork)
        #[arg(long)]
        cellular: bool,
    },

    /// Busca en los addons habilitados (en paralelo)
    Search {
        /// texto a buscar
        query: String,
        /// limitar a un addon
        #[arg(short, long)]
        addon: Option<String>,
        /// cuántos resultados mostrar por tipo
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },

    /// Resuelve y muestra la URL de audio de un tema
    Stream {
        /// id del tema dentro del addon
        track_id: String,
        #[arg(short, long)]
        addon: Option<String>,
    },

    /// Diagnóstico completo de la cadena de reproducción de un addon
    Doctor {
        /// texto de búsqueda para hacer la prueba
        query: String,
        /// limitar a un addon (si no, usa el primero habilitado)
        #[arg(short, long)]
        addon: Option<String>,
    },

    /// Vuelca el JSON crudo de un endpoint del addon (debug de protocolo)
    Dump {
        /// path del endpoint, p.ej. "search", "stream/12345", "album/678"
        path: String,
        /// query params extra, en formato clave=valor (repetible)
        #[arg(short, long)]
        q: Vec<String>,
        #[arg(short, long)]
        addon: Option<String>,
    },

    /// Reproduce un tema por consola (Ctrl-C para salir)
    Play {
        track_id: String,
        #[arg(short, long)]
        addon: Option<String>,
    },

    /// Muestra una fila de catálogo declarada por el addon
    Catalog {
        /// id del addon
        addon: String,
        /// id del catálogo (p.ej. top)
        catalog: String,
        /// cuántos ítems saltear (múltiplo de 100)
        #[arg(long, default_value_t = 0)]
        skip: u32,
    },

    /// Muestra el detalle de un álbum / artista / playlist
    Detail {
        /// album | artist | playlist
        kind: String,
        /// id del addon
        addon: String,
        /// id del recurso
        id: String,
    },

    /// Refresca los manifests de todos los addons
    Refresh,

    /// Muestra rutas, caché y estado del motor de audio
    Info,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    if cli.no_mpv {
        tracing::warn!(
            "--no-mpv es una opción de compilación (SONIDO_NO_MPV=1 cargo build); \
             en runtime se usa libmpv si el binario se compiló con soporte"
        );
    }

    match cli.cmd {
        None | Some(Cmd::Gui) => run_gui(cli),
        Some(cmd) => run_cli(cmd),
    }
}

fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "warn,sonido=info",
        1 => "info,sonido=debug",
        2 => "debug,sonido=trace",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init();
}

fn run_cli(cmd: Cmd) -> anyhow::Result<()> {
    let h = sonido::headless::Headless::new();
    match cmd {
        Cmd::Gui => unreachable!(),
        Cmd::Add { url } => {
            h.install(&url)?;
        }
        Cmd::List => h.list(),
        Cmd::Remove { id_or_url } => h.remove(&id_or_url)?,
        Cmd::Set {
            addon,
            key,
            value,
            cellular,
        } => h.set_setting(&addon, &key, &value, cellular)?,
        Cmd::Search { query, addon, limit } => h.search(addon.as_deref(), &query, limit)?,
        Cmd::Stream { track_id, addon } => h.stream(addon.as_deref(), &track_id)?,
        Cmd::Doctor { query, addon } => h.doctor(addon.as_deref(), &query)?,
        Cmd::Dump { path, q, addon } => {
            let owned: Vec<(String, String)> = q
                .iter()
                .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            let refs: Vec<(&str, String)> =
                owned.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            h.dump(addon.as_deref(), &path, &refs)?;
        }
        Cmd::Play { track_id, addon } => h.play(addon.as_deref(), &track_id)?,
        Cmd::Catalog {
            addon,
            catalog,
            skip,
        } => h.catalog(&addon, &catalog, skip)?,
        Cmd::Detail { kind, addon, id } => h.detail(&addon, &kind, &id)?,
        Cmd::Refresh => {
            let net = sonido::net::NetworkKind::detect();
            let n = h.reg.refresh_manifests(net)?;
            println!("✔ {n} manifest(s) actualizado(s)");
            h.list();
        }
        Cmd::Info => {
            println!("config    : {}", sonido::config::config_dir().display());
            println!("datos     : {}", sonido::config::data_dir().display());
            println!("caché     : {}", sonido::config::cache_dir().display());
            let cache = std::sync::Arc::new(sonido::http::DiskCache::new(
                sonido::config::cache_dir().join("http"),
            ));
            println!(
                "caché http: {} en disco",
                sonido::util::fmt_bytes(cache.size_bytes())
            );
            println!("addons    : {}", h.reg.count());
            h.list();
            println!("red       : {}", h.net.label());
            #[cfg(mpv)]
            println!("motor     : libmpv disponible (compilado con soporte)");
            #[cfg(not(mpv))]
            println!("motor     : rodio (sin soporte libmpv en esta compilación)");
        }
    }
    h.store.save();
    Ok(())
}

/// OpenGL (glow) por defecto; wgpu/Vulkan solo si se compila con `--features wgpu_renderer`.
fn pick_renderer(want_glow: bool) -> eframe::Renderer {
    #[cfg(feature = "wgpu_renderer")]
    {
        if !want_glow {
            return eframe::Renderer::Wgpu;
        }
    }
    #[cfg(not(feature = "wgpu_renderer"))]
    {
        if !want_glow {
            tracing::info!("renderer: OpenGL (compilá con --features wgpu_renderer para Vulkan)");
        }
    }
    let _ = want_glow;
    eframe::Renderer::default()
}

fn run_gui(cli: Cli) -> anyhow::Result<()> {
    let w = cli.width;
    let h = cli.height;
    let use_glow = cli.renderer.eq_ignore_ascii_case("glow") || cli.renderer.eq_ignore_ascii_case("gl");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([w as f32, h as f32])
            .with_min_inner_size([900.0, 560.0])
            .with_title("Sonido")
            .with_app_id("sonido")
            .with_fullsize_content_view(true)
            .with_transparent(false),
        renderer: pick_renderer(use_glow),
        ..Default::default()
    };

    eframe::run_native(
        "Sonido",
        options,
        Box::new(|cc| {
            Ok(Box::new(SonidoApp {
                app: sonido::ui::app::App::new(cc)?,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("no se pudo abrir la ventana: {e}"))?;
    Ok(())
}

/// Adaptador al trait `eframe::App` (en eframe 0.36 el punto de entrada es `ui()`).
struct SonidoApp {
    app: sonido::ui::app::App,
}

impl eframe::App for SonidoApp {
    /// Lógica sin pintar: drenar eventos de red/audio y preparar el gapless.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.poll_events(ctx);
        self.app.maybe_preroll();
        self.app.svc.store.flush_if_needed(false);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // por si `logic` no corrió (primer frame)
        self.app.poll_events(&ctx);
        sonido::ui::shell::ui(&mut self.app, ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        sonido::ui::theme::BG.to_normalized_gamma_f32()
    }

}
