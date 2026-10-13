//! Panel nativo de **NuCoder** en NuIde (Slint).
//!
//! Corre el agente del núcleo `nucoder` (UI-agnóstico) en un hilo y traduce sus
//! `AgentEvent` a un modelo de chat para la UI. Además hace de **host de
//! terminales**: los pedidos `run_in_terminal` del agente se atienden desde
//! `Ide::drain`, que escribe el comando en una terminal real de la IDE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex;

use serde_json::Value;
use slint::{ModelRc, SharedString, StyledText, VecModel};

use nucoder::agent::{self, Agent, AgentEvent};
use nucoder::caps::{self, Cap};
use nucoder::config::{Config, CuratedModel, TASKS};
use nucoder::image::Image;
use nucoder::knowledge::Knowledge;
use nucoder::markdown::{self, Segment, TokenKind};
use nucoder::mcp::McpHub;
use nucoder::session;
use nucoder::skills::Registry;
use nucoder::tools::{FtpJob, TerminalJob};

use crate::ui::{IdeChatCodeLine, IdeChatModel, IdeChatMsg, IdeChatSeg, IdeModelRow, IdeToken};

/// Un mensaje del transcript (texto crudo: se segmenta al dibujar).
struct ChatMsg {
    /// 0 usuario · 1 asistente · 2 herramienta · 3 aviso · 4 error.
    role: i32,
    title: String,
    text: String,
}

/// Una fila del listado de modelos (todos los configurados, con su especialidad).
struct ModelRow {
    id: String,
    label: String,
    provider: String,
    specialty: String,
    /// Grupo (primera capacidad específica) para las secciones.
    group: String,
    free: bool,
    power: u8,
    chat: bool,
}

/// Orden canónico de las especialidades (para las secciones).
fn specialty_rank(group: &str) -> usize {
    const ORDER: [&str; 8] = [
        "coder",
        "visión",
        "imagen",
        "voz/audio",
        "video",
        "embedding",
        "guard",
        "texto",
    ];
    ORDER.iter().position(|o| *o == group).unwrap_or(9)
}

/// Una fila del listado ya agrupado: cabecera de sección o modelo.
enum DisplayRow<'a> {
    Header(String),
    Model(&'a ModelRow),
}

/// Clave de orden por ping: probado OK = su ms; sin probar/err/probando = MAX.
fn ping_key(state: Option<&PingState>) -> u64 {
    match state {
        Some(PingState::Ok(ms)) => *ms,
        _ => u64::MAX,
    }
}

/// Estado del ping de un modelo.
enum PingState {
    Pinging,
    Ok(u64),
    Err(String),
}

/// Estado del panel.
pub struct NuCoderPanel {
    open: bool,
    cwd: PathBuf,
    config: Arc<Config>,
    knowledge: Arc<Knowledge>,
    skills: Arc<Registry>,
    /// Conexiones MCP persistentes entre turnos del agente.
    mcp_hub: Arc<Mutex<McpHub>>,
    task: String,
    free_only: bool,
    cap: Option<Cap>,
    model_override: Option<String>,
    sandbox: bool,
    /// `confirm` de la config (base); el switch `auto_approve` lo pisa.
    confirm: bool,
    auto_approve: bool,
    /// Permisos (pestaña Permisos).
    perm_web: bool,
    perm_read_outside: bool,
    perm_terminal: bool,
    perm_ftp: bool,
    autopilot: bool,
    prompt_cache: bool,
    context_chars: usize,
    conversation: Vec<Value>,
    images: Vec<Image>,
    tx: Sender<AgentEvent>,
    rx: Receiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
    running: bool,
    model_label: String,
    /// Modelos de chat curados para el selector.
    models: Vec<CuratedModel>,
    msgs: Vec<ChatMsg>,
    /// Índice del mensaje del asistente que se está streameando.
    streaming: Option<usize>,
    /// Confirmación pendiente: (nombre, detalle, respuesta).
    pending: Option<(String, String, Sender<bool>)>,
    /// Mensajes del agente pidiendo correr en una terminal de la IDE.
    terminal_tx: Sender<TerminalJob>,
    terminal_rx: Receiver<TerminalJob>,
    /// Pedidos del agente al cliente FTP de la IDE.
    ftp_tx: Sender<FtpJob>,
    ftp_rx: Receiver<FtpJob>,
    /// El modelo de la UI hay que reconstruirlo.
    dirty: bool,
    /// Hay que reenviar las listas (modelos curados, tareas).
    lists_dirty: bool,
    /// Pulso del icono «pensando» (lo alterna la IDE).
    pulse: bool,
    /// Listado de modelos (pestaña «modelos»).
    model_rows: Vec<ModelRow>,
    model_filter: String,
    ping: HashMap<String, PingState>,
    ping_tx: Sender<(String, Result<u64, String>)>,
    ping_rx: Receiver<(String, Result<u64, String>)>,
    /// Orden del listado: 0 especialidad · 1 ping · 2 nombre · 3 gratis.
    model_sort: u8,
    /// Canal del hilo que re-baja el catálogo («sincronizar»).
    sync_tx: Sender<String>,
    sync_rx: Receiver<String>,
}

impl NuCoderPanel {
    pub fn new(cwd: PathBuf) -> Self {
        let config = Config::load();
        let knowledge = Arc::new(Knowledge::discover(&config.knowledge));
        let skills = Arc::new(Registry::discover(
            &cwd,
            &config.skills,
            &config.skills_permissions,
            config.skills_autoinvoke,
        ));
        let mcp_hub = Arc::new(Mutex::new(McpHub::default()));
        let (tx, rx) = channel();
        let (terminal_tx, terminal_rx) = channel();
        let (ftp_tx, ftp_rx) = channel();
        let (ping_tx, ping_rx) = channel();
        let (sync_tx, sync_rx) = channel();
        let models = config.curated_models();
        let mut panel = Self {
            open: false,
            cwd,
            confirm: config.confirm,
            auto_approve: config.auto_approve,
            perm_web: config.perm_web,
            perm_read_outside: config.perm_read_outside,
            perm_terminal: config.perm_terminal,
            perm_ftp: config.perm_ftp,
            autopilot: config.autopilot,
            free_only: config.free_only,
            sandbox: config.sandbox,
            prompt_cache: config.prompt_cache,
            context_chars: config.context_chars,
            task: "complete".to_string(),
            cap: None,
            model_override: config.default_model.clone(),
            knowledge,
            skills,
            mcp_hub,
            config: Arc::new(config),
            conversation: Vec::new(),
            images: Vec::new(),
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            running: false,
            model_label: "auto (router)".to_string(),
            models,
            msgs: Vec::new(),
            streaming: None,
            pending: None,
            terminal_tx,
            terminal_rx,
            ftp_tx,
            ftp_rx,
            dirty: true,
            lists_dirty: true,
            pulse: false,
            model_rows: Vec::new(),
            model_filter: String::new(),
            ping: HashMap::new(),
            ping_tx,
            ping_rx,
            model_sort: 0,
            sync_tx,
            sync_rx,
        };
        panel.rebuild_model_rows();
        panel.push(
            3,
            "NuCoder",
            "Listo. Escribí una pregunta; usá el switch **auto-aprobar** para que no pida permiso en cada paso.".to_string(),
        );
        panel
    }

    // ---------------------------------------------------------------- estado
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn running(&self) -> bool {
        self.running
    }
    pub fn model_label(&self) -> &str {
        &self.model_label
    }
    pub fn task(&self) -> &str {
        &self.task
    }
    pub fn free_only(&self) -> bool {
        self.free_only
    }
    pub fn auto_approve(&self) -> bool {
        self.auto_approve
    }
    pub fn perm_web(&self) -> bool {
        self.perm_web
    }
    pub fn perm_read_outside(&self) -> bool {
        self.perm_read_outside
    }
    pub fn perm_terminal(&self) -> bool {
        self.perm_terminal
    }
    pub fn perm_ftp(&self) -> bool {
        self.perm_ftp
    }
    pub fn autopilot(&self) -> bool {
        self.autopilot
    }

    /// Prende/apaga un permiso y lo persiste en la config.
    pub fn toggle_permission(&mut self, which: &str) {
        match which {
            "web" => self.perm_web = !self.perm_web,
            "read-outside" => self.perm_read_outside = !self.perm_read_outside,
            "terminal" => self.perm_terminal = !self.perm_terminal,
            "ftp" => self.perm_ftp = !self.perm_ftp,
            "autopilot" => self.autopilot = !self.autopilot,
            "sandbox" => self.sandbox = !self.sandbox,
            "confirm" => self.confirm = !self.confirm,
            _ => return,
        }
        let mut cfg = Config::load();
        cfg.perm_web = self.perm_web;
        cfg.perm_read_outside = self.perm_read_outside;
        cfg.perm_terminal = self.perm_terminal;
        cfg.perm_ftp = self.perm_ftp;
        cfg.autopilot = self.autopilot;
        cfg.sandbox = self.sandbox;
        cfg.confirm = self.confirm;
        let _ = cfg.save_own();
        let estado = |b: bool| if b { "ON" } else { "OFF" };
        let msg = match which {
            "web" => format!("internet: {}", estado(self.perm_web)),
            "read-outside" => format!("leer fuera del proyecto: {}", estado(self.perm_read_outside)),
            "terminal" => format!("terminales de la IDE: {}", estado(self.perm_terminal)),
            "autopilot" => format!("autopiloto: {}", estado(self.autopilot)),
            "sandbox" => format!("sandbox de escrituras: {}", estado(self.sandbox)),
            _ => format!("confirmación: {}", estado(self.confirm)),
        };
        self.info(msg);
    }

    pub fn sandbox(&self) -> bool {
        self.sandbox
    }
    pub fn confirm(&self) -> bool {
        self.confirm
    }
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Etiqueta del tope de contexto (p. ej. `48k`).
    pub fn context_label(&self) -> String {
        format!("{}k", self.context_chars / 1000)
    }

    /// Adjunta una imagen (para el turno con **visión**). Sólo una por turno,
    /// como la TUI.
    pub fn add_image(&mut self, path: &Path) {
        if !self.images.is_empty() {
            self.info("ya hay una imagen adjunta (una por turno)".to_string());
            return;
        }
        match nucoder::image::load(path) {
            Ok(im) => {
                let label = im.label.clone();
                self.images.push(im);
                self.push(3, "imagen", format!("adjuntada: {label}"));
            }
            Err(e) => self.push(4, "error", format!("no se pudo cargar la imagen: {e}")),
        }
    }

    pub fn clear_images(&mut self) {
        let n = self.images.len();
        self.images.clear();
        if n > 0 {
            self.info(format!("imágenes quitadas ({n})"));
        }
    }

    /// Cicla el tope de contexto y lo **persiste** en la config.
    pub fn cycle_context(&mut self) {
        const STEPS: [usize; 5] = [16_000, 32_000, 48_000, 96_000, 128_000];
        let idx = STEPS
            .iter()
            .position(|s| *s >= self.context_chars)
            .unwrap_or(STEPS.len() - 1);
        self.context_chars = STEPS[(idx + 1) % STEPS.len()];
        let n = self.context_chars;
        self.info(format!("tope de contexto → {n} caracteres"));
        let mut cfg = Config::load();
        cfg.context_chars = n;
        let _ = cfg.save_own();
    }

    /// Etiqueta del modelo a mostrar: el forzado (si hay) o el ruteado/auto.
    pub fn display_model(&self) -> String {
        match &self.model_override {
            Some(id) => self
                .models
                .iter()
                .find(|m| &m.id == id)
                .map(|m| m.label.clone())
                .unwrap_or_else(|| id.clone()),
            None => self.model_label.clone(),
        }
    }

    /// Índice del modelo forzado en la lista curada (-1 = auto).
    pub fn model_index(&self) -> i32 {
        match &self.model_override {
            Some(id) => self
                .models
                .iter()
                .position(|m| &m.id == id)
                .map(|i| i as i32)
                .unwrap_or(-1),
            None => -1,
        }
    }

    pub fn models_model(&self) -> ModelRc<IdeChatModel> {
        let rows: Vec<IdeChatModel> = self
            .models
            .iter()
            .map(|m| IdeChatModel {
                id: m.id.clone().into(),
                label: m.label.clone().into(),
                free: m.free,
                power: m.power as i32,
                caps: m
                    .caps
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
                    .into(),
            })
            .collect();
        ModelRc::from(std::rc::Rc::new(VecModel::from(rows)))
    }

    // --- listado de modelos (pestaña «modelos») ---

    fn rebuild_model_rows(&mut self) {
        self.model_rows = self
            .config
            .providers
            .iter()
            .flat_map(|p| {
                p.models.iter().map(move |m| ModelRow {
                    id: m.id.clone(),
                    label: format!("{}/{}", p.id, m.name),
                    provider: p.id.clone(),
                    specialty: caps::specialty(&m.caps),
                    group: m
                        .caps
                        .iter()
                        .find(|c| **c != Cap::Text)
                        .map(|c| c.label().to_string())
                        .unwrap_or_else(|| "texto".to_string()),
                    free: m.free,
                    power: m.power,
                    chat: m.caps.contains(&Cap::Text),
                })
            })
            .collect();
    }

    /// Filas del listado filtrado, en el orden elegido (agrupado en «especialidad»).
    fn display_rows(&self) -> Vec<DisplayRow<'_>> {
        let f = self.model_filter.trim().to_lowercase();
        let mut models: Vec<&ModelRow> = self
            .model_rows
            .iter()
            .filter(|r| {
                f.is_empty()
                    || r.label.to_lowercase().contains(&f)
                    || r.specialty.to_lowercase().contains(&f)
                    || r.id.to_lowercase().contains(&f)
            })
            .collect();
        let mut out = Vec::new();
        match self.model_sort {
            // Agrupado por especialidad.
            0 => {
                models.sort_by(|a, b| {
                    specialty_rank(&a.group)
                        .cmp(&specialty_rank(&b.group))
                        .then((!a.free).cmp(&(!b.free)))
                        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
                });
                let mut last: Option<&str> = None;
                for m in models {
                    if last != Some(m.group.as_str()) {
                        out.push(DisplayRow::Header(m.group.clone()));
                        last = Some(m.group.as_str());
                    }
                    out.push(DisplayRow::Model(m));
                }
            }
            // Por ping (los más rápidos primero; sin probar/err al final).
            1 => {
                models.sort_by(|a, b| {
                    ping_key(self.ping.get(&a.id))
                        .cmp(&ping_key(self.ping.get(&b.id)))
                        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
                });
                out.extend(models.into_iter().map(DisplayRow::Model));
            }
            // Por nombre.
            2 => {
                models.sort_by_cached_key(|a| a.label.to_lowercase());
                out.extend(models.into_iter().map(DisplayRow::Model));
            }
            // Gratis primero.
            _ => {
                models.sort_by(|a, b| {
                    (!a.free)
                        .cmp(&(!b.free))
                        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
                });
                out.extend(models.into_iter().map(DisplayRow::Model));
            }
        }
        out
    }

    /// Cambia el orden del listado (cicla).
    pub fn model_sort_cycle(&mut self) {
        self.model_sort = (self.model_sort + 1) % 4;
    }

    pub fn model_sort_label(&self) -> &'static str {
        match self.model_sort {
            0 => "especialidad",
            1 => "ping",
            2 => "nombre",
            _ => "gratis",
        }
    }

    /// Re-baja el catálogo de todos los proveedores en un hilo y avisa al volver.
    pub fn sync_models(&mut self) {
        self.info("sincronizando el catálogo…".to_string());
        let tx = self.sync_tx.clone();
        std::thread::spawn(move || {
            let mut cfg = nucoder::config::Config::load();
            let results = nucoder::config::refresh_catalog(&mut cfg, None, false);
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let total: usize = results.iter().filter_map(|(_, r)| r.as_ref().ok().copied()).sum();
            let errs: Vec<String> = results
                .iter()
                .filter_map(|(id, r)| r.as_ref().err().map(|e| format!("{id}: {e}")))
                .collect();
            let msg = match cfg.save_own() {
                Ok(_) => {
                    let extra = if errs.is_empty() {
                        String::new()
                    } else {
                        format!(" · errores: {}", errs.join("; "))
                    };
                    format!("catálogo sincronizado: {ok} proveedor(es), {total} modelo(s){extra}")
                }
                Err(e) => format!("se bajó el catálogo pero no se pudo guardar: {e}"),
            };
            let _ = tx.send(msg);
        });
    }

    /// Drena el aviso de «sincronizar». `true` si llegó algo.
    pub fn poll_sync(&mut self) -> bool {
        if let Ok(msg) = self.sync_rx.try_recv() {
            self.config = Arc::new(nucoder::config::Config::load());
            self.models = self.config.curated_models();
            self.rebuild_model_rows();
            self.info(msg);
            true
        } else {
            false
        }
    }

    pub fn model_filter_set(&mut self, value: &str) {
        self.model_filter = value.to_string();
    }

    pub fn model_filter(&self) -> &str {
        &self.model_filter
    }

    pub fn models_rows(&self) -> ModelRc<IdeModelRow> {
        let rows: Vec<IdeModelRow> = self
            .display_rows()
            .iter()
            .map(|d| match d {
                DisplayRow::Header(group) => IdeModelRow {
                    id: SharedString::default(),
                    label: group.clone().into(),
                    provider: SharedString::default(),
                    specialty: SharedString::default(),
                    free: false,
                    power: 0,
                    status: SharedString::default(),
                    state: 0,
                    chat: false,
                    header: true,
                },
                DisplayRow::Model(r) => {
                    let (status, state) = match self.ping.get(&r.id) {
                        Some(PingState::Pinging) => ("…".to_string(), 1),
                        Some(PingState::Ok(ms)) => (format!("{ms} ms"), 2),
                        Some(PingState::Err(e)) => (
                            if e.trim().is_empty() {
                                "error".to_string()
                            } else {
                                e.clone()
                            },
                            3,
                        ),
                        None => (String::new(), 0),
                    };
                    IdeModelRow {
                        id: r.id.clone().into(),
                        label: r.label.clone().into(),
                        provider: r.provider.clone().into(),
                        specialty: r.specialty.clone().into(),
                        free: r.free,
                        power: r.power as i32,
                        status: status.into(),
                        state,
                        chat: r.chat,
                        header: false,
                    }
                }
            })
            .collect();
        ModelRc::from(std::rc::Rc::new(VecModel::from(rows)))
    }

    /// Probar (ping) el modelo en la posición `index` del listado agrupado.
    pub fn ping_at(&mut self, index: i32) {
        let id = match self.display_rows().get(index as usize) {
            Some(DisplayRow::Model(r)) => r.id.clone(),
            _ => return,
        };
        self.ping_model(&id);
    }

    /// Probar **todos** los modelos.
    pub fn ping_all(&mut self) {
        let ids: Vec<String> = self.model_rows.iter().map(|r| r.id.clone()).collect();
        for id in ids {
            self.ping_model(&id);
        }
    }

    /// Forzar el modelo en la posición `index` del listado agrupado.
    pub fn use_model_at(&mut self, index: i32) {
        let id = match self.display_rows().get(index as usize) {
            Some(DisplayRow::Model(r)) => r.id.clone(),
            _ => return,
        };
        self.model_override = Some(id.clone());
        let mut cfg = Config::load();
        cfg.default_model = Some(id.clone());
        let _ = cfg.save_own();
        self.info(format!("modelo forzado: {id}"));
    }

    fn ping_model(&mut self, id: &str) {
        let Some((base, key)) = self.provider_of(id) else {
            return;
        };
        let model = id.to_string();
        self.ping.insert(model.clone(), PingState::Pinging);
        let tx = self.ping_tx.clone();
        std::thread::spawn(move || {
            let res = nucoder::client::ping(&base, &key, &model).map_err(|e| e.to_string());
            let _ = tx.send((model, res));
        });
    }

    fn provider_of(&self, model_id: &str) -> Option<(String, String)> {
        self.config
            .providers
            .iter()
            .find(|p| p.models.iter().any(|m| m.id == model_id))
            .map(|p| (p.base_url.clone(), p.api_key.clone()))
    }

    /// Drena los resultados de ping. `true` si cambió alguno.
    pub fn poll_pings(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, res)) = self.ping_rx.try_recv() {
            self.ping.insert(
                id,
                match res {
                    Ok(ms) => PingState::Ok(ms),
                    Err(e) => PingState::Err(e),
                },
            );
            changed = true;
        }
        changed
    }

    pub fn tasks_model(&self) -> ModelRc<SharedString> {
        ModelRc::from(std::rc::Rc::new(VecModel::from(
            TASKS
                .iter()
                .map(|t| SharedString::from(*t))
                .collect::<Vec<_>>(),
        )))
    }

    pub fn task_index(&self) -> i32 {
        TASKS
            .iter()
            .position(|t| *t == self.task)
            .map(|i| i as i32)
            .unwrap_or(0)
    }

    /// Fija la tarea por índice (`config::TASKS`).
    pub fn set_task(&mut self, index: i32) {
        if let Some(t) = usize::try_from(index).ok().and_then(|i| TASKS.get(i)) {
            self.task = (*t).to_string();
            let tarea = self.task.clone();
            self.info(format!("tarea → {tarea}"));
        }
    }

    /// Fuerza un modelo por índice curado (`-1` = auto) y lo persiste en
    /// `default_model`.
    pub fn set_model(&mut self, index: i32) {
        let id = if index < 0 {
            None
        } else {
            match self.models.get(index as usize) {
                Some(m) => Some(m.id.clone()),
                None => return,
            }
        };
        self.model_override = id.clone();
        let mut cfg = Config::load();
        cfg.default_model = id.clone();
        let _ = cfg.save_own();
        match id {
            Some(id) => self.info(format!("modelo forzado: {id}")),
            None => self.info("modelo: auto (router)".to_string()),
        }
    }

    /// (pendiente, título, detalle) de una confirmación.
    pub fn pending_info(&self) -> (bool, String, String) {
        match &self.pending {
            Some((name, detail, _)) => (true, name.clone(), detail.clone()),
            None => (false, String::new(), String::new()),
        }
    }

    /// Abre el panel apuntando al proyecto `cwd` y relee la config.
    pub fn open(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        self.config = Arc::new(Config::load());
        self.models = self.config.curated_models();
        self.rebuild_model_rows();
        self.knowledge = Arc::new(Knowledge::discover(&self.config.knowledge));
        self.skills = Arc::new(Registry::discover(
            &self.cwd,
            &self.config.skills,
            &self.config.skills_permissions,
            self.config.skills_autoinvoke,
        ));
        // Cambia el proyecto: se reconectan los servers MCP (cwd nuevo).
        self.mcp_hub = Arc::new(Mutex::new(McpHub::default()));
        self.confirm = self.config.confirm;
        self.auto_approve = self.config.auto_approve;
        self.perm_web = self.config.perm_web;
        self.perm_read_outside = self.config.perm_read_outside;
        self.perm_terminal = self.config.perm_terminal;
        self.perm_ftp = self.config.perm_ftp;
        self.autopilot = self.config.autopilot;
        self.free_only = self.config.free_only;
        self.sandbox = self.config.sandbox;
        self.prompt_cache = self.config.prompt_cache;
        self.context_chars = self.config.context_chars;
        self.open = true;
        self.dirty = true;
        self.lists_dirty = true;
        if self.conversation.is_empty() {
            if let Some(msgs) = session::load(&self.cwd) {
                let n = msgs.len();
                self.conversation = msgs;
                self.info(format!("sesión previa cargada ({n} mensajes) · usá «nueva» para reiniciar"));
            }
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Prende/apaga el auto-aprobar y lo **persiste** en la config.
    pub fn toggle_auto_approve(&mut self) {
        self.auto_approve = !self.auto_approve;
        let estado = if self.auto_approve { "ON" } else { "OFF" };
        self.info(format!("auto-aprobar: {estado}"));
        let mut cfg = Config::load();
        cfg.auto_approve = self.auto_approve;
        let _ = cfg.save_own();
    }

    pub fn toggle_free(&mut self) {
        self.free_only = !self.free_only;
        let estado = if self.free_only { "ON" } else { "OFF" };
        self.info(format!("sólo gratis: {estado}"));
    }

    // ---------------------------------------------------------------- acciones
    pub fn clear(&mut self) {
        if self.running {
            return;
        }
        self.msgs.clear();
        self.conversation.clear();
        self.images.clear();
        self.streaming = None;
        session::remove(&self.cwd);
        self.push(3, "NuCoder", "conversación reiniciada".to_string());
    }

    pub fn cancel(&mut self) {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.info("cancelando…".to_string());
        }
    }

    pub fn answer(&mut self, ok: bool) {
        if let Some((name, _, reply)) = self.pending.take() {
            let _ = reply.send(ok);
            let (tag, role) = if ok { ("aprobado", 3) } else { ("rechazado", 4) };
            self.push(role, name, tag.to_string());
        }
    }

    pub fn send(&mut self, prompt: &str) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() || self.running {
            return;
        }
        // Menciones `@skill`: se cargan en el turno (sin esperar la herramienta).
        let (prompt, mention_ctx, loaded, missing) = self.skills.expand_mentions(&prompt);
        if prompt.is_empty() && mention_ctx.is_empty() {
            return;
        }
        if !loaded.is_empty() {
            self.info(format!("skills precargados: {}", loaded.join(", ")));
        }
        if !missing.is_empty() {
            self.info(format!("skills no encontrados: {}", missing.join(", ")));
        }
        self.push(0, "vos".to_string(), prompt.clone());
        let full = if mention_ctx.is_empty() {
            prompt.clone()
        } else {
            format!("{mention_ctx}\n\n---\n{prompt}")
        };
        let user_msg = agent::user_message(&full, &self.images);
        let vision = !self.images.is_empty();
        self.images.clear();
        if self.conversation.is_empty() {
            let system = agent::system_value(
                &agent::system_prompt(&self.cwd, &self.knowledge, &self.skills, self.autopilot),
                self.prompt_cache,
            );
            self.conversation = agent::new_conversation_with(system, user_msg);
        } else {
            self.conversation.push(user_msg);
        }
        self.running = true;
        self.streaming = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let agent = Agent {
            config: self.config.clone(),
            cwd: self.cwd.clone(),
            task: self.task.clone(),
            free_only: self.free_only,
            cancel: self.cancel.clone(),
            confirm: self.confirm,
            sandbox: self.sandbox,
            knowledge: self.knowledge.clone(),
            skills: self.skills.clone(),
            mcp_hub: self.mcp_hub.clone(),
            cap: self.cap,
            vision,
            model_override: self.model_override.clone(),
            context_chars: self.context_chars,
            terminal: Some(self.terminal_tx.clone()),
            auto_approve: self.auto_approve,
            autopilot: self.autopilot,
            web: self.perm_web,
            read_outside: self.perm_read_outside,
            terminal_allowed: self.perm_terminal,
            ftp: Some(self.ftp_tx.clone()),
            ftp_allowed: self.perm_ftp,
        };
        let tx = self.tx.clone();
        let conv = std::mem::take(&mut self.conversation);
        std::thread::spawn(move || agent.run(conv, tx));
    }

    // ---------------------------------------------------------------- canales
    /// Drena los eventos del agente. Devuelve `true` si algo cambió.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            self.on_event(ev);
            changed = true;
        }
        changed
    }

    /// Saco los pedidos de terminal pendientes (los atiende `Ide`).
    pub fn take_terminal_jobs(&mut self) -> Vec<TerminalJob> {
        let mut out = Vec::new();
        while let Ok(job) = self.terminal_rx.try_recv() {
            out.push(job);
        }
        out
    }

    /// Saco los pedidos de FTP pendientes (los atiende `Ide`).
    pub fn take_ftp_jobs(&mut self) -> Vec<FtpJob> {
        let mut out = Vec::new();
        while let Ok(job) = self.ftp_rx.try_recv() {
            out.push(job);
        }
        out
    }

    // ---------------------------------------------------------------- modelo
    /// Modelo de chat para Slint (lo llama `Ide::sync` cuando cambió).
    pub fn slint_messages(&self) -> ModelRc<IdeChatMsg> {
        let rows: Vec<IdeChatMsg> = self.msgs.iter().map(msg_to_slint).collect();
        ModelRc::from(std::rc::Rc::new(VecModel::from(rows)))
    }

    /// ¿Hay que reconstruir el modelo? (consume el flag).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// ¿Hay que reenviar las listas (modelos/tareas)? (consume el flag).
    pub fn take_lists_dirty(&mut self) -> bool {
        std::mem::take(&mut self.lists_dirty)
    }

    /// Alterna el pulso del icono «pensando» (lo llama el timer de la IDE).
    pub fn tick(&mut self) {
        self.pulse = !self.pulse;
    }

    /// Estado actual del pulso.
    pub fn pulse(&self) -> bool {
        self.pulse
    }

    // ---------------------------------------------------------------- interno
    fn push(&mut self, role: i32, title: impl Into<String>, text: String) {
        self.msgs.push(ChatMsg {
            role,
            title: title.into(),
            text,
        });
        if self.msgs.len() > 400 {
            self.msgs.remove(0);
        }
        self.dirty = true;
    }

    fn info(&mut self, t: impl Into<String>) {
        self.push(3, "·", t.into());
    }

    fn on_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Routed { label, free } => {
                self.model_label = label.clone();
                self.streaming = None;
                let tag = if free { "free" } else { "pago" };
                self.info(format!("modelo [{tag}]: {label}"));
            }
            AgentEvent::Delta(t) => match self.streaming {
                Some(i) if i < self.msgs.len() => {
                    self.msgs[i].text.push_str(&t);
                    self.dirty = true;
                }
                _ => {
                    self.push(1, self.model_label.clone(), t);
                    self.streaming = Some(self.msgs.len() - 1);
                }
            },
            AgentEvent::ToolStart { name, args } => {
                self.streaming = None;
                self.push(2, format!("🔧 {name}"), preview(&args, 400));
            }
            AgentEvent::ToolResult { name, text } => {
                self.streaming = None;
                self.push(3, format!("↳ {name}"), preview(&text, 800));
            }
            AgentEvent::Notice(m) => {
                self.streaming = None;
                self.info(m);
            }
            AgentEvent::Error(m) => {
                self.streaming = None;
                self.push(4, "error", m);
            }
            AgentEvent::Confirm { name, args, reply } => {
                self.streaming = None;
                self.pending = Some((name.clone(), preview(&args, 200), reply));
                self.dirty = true;
            }
            AgentEvent::Plan { goal, steps, reply } => {
                self.streaming = None;
                let mut text = String::new();
                if !goal.trim().is_empty() {
                    text.push_str(&format!("**Plan:** {goal}\n\n"));
                }
                for (i, s) in steps.iter().enumerate() {
                    text.push_str(&format!("{}. {s}\n", i + 1));
                }
                self.push(2, "plan", text);
                self.pending = Some(("plan".to_string(), goal, reply));
                self.dirty = true;
            }
            AgentEvent::Done { conversation } => {
                self.streaming = None;
                self.running = false;
                self.conversation = conversation;
                let _ = session::save(&self.cwd, &self.conversation);
                self.dirty = true;
            }
        }
    }
}

/// Convierte un mensaje interno al struct de Slint (con los segmentos).
fn msg_to_slint(m: &ChatMsg) -> IdeChatMsg {
    let segs: Vec<IdeChatSeg> = markdown::segments(&m.text)
        .into_iter()
        .map(|s| match s {
            Segment::Prose(p) => IdeChatSeg {
                code: false,
                lang: SharedString::default(),
                md: crate::editor::markdown_preview(&p),
                raw: SharedString::default(),
                lines: ModelRc::default(),
            },
            Segment::Code(cb) => {
                let raw: String = cb
                    .lines
                    .iter()
                    .map(|line| line.iter().map(|t| t.text.as_str()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n");
                IdeChatSeg {
                    code: true,
                    lang: cb.lang.into(),
                    md: StyledText::default(),
                    raw: raw.into(),
                    lines: ModelRc::from(std::rc::Rc::new(VecModel::from(
                        cb.lines
                            .into_iter()
                            .map(|line| IdeChatCodeLine {
                                tokens: ModelRc::from(std::rc::Rc::new(VecModel::from(
                                    line.into_iter()
                                        .map(|t| IdeToken {
                                            text: t.text.into(),
                                            kind: token_kind(t.kind),
                                        })
                                        .collect::<Vec<_>>(),
                                ))),
                            })
                            .collect::<Vec<_>>(),
                    ))),
                }
            }
        })
        .collect();
    IdeChatMsg {
        role: m.role,
        title: m.title.clone().into(),
        text: m.text.clone().into(),
        segs: ModelRc::from(std::rc::Rc::new(VecModel::from(segs))),
    }
}

/// Categoría de token del núcleo → kind de la UI (el mismo del editor).
fn token_kind(k: TokenKind) -> i32 {
    match k {
        TokenKind::Plain => 0,
        TokenKind::Comment => 1,
        TokenKind::Keyword => 2,
        TokenKind::Str => 3,
        TokenKind::Number => 4,
        TokenKind::Type => 5,
        TokenKind::Func => 5,
    }
}

/// Colapsa saltos de línea y recorta, para los detalles de tool/confirmación.
fn preview(s: &str, max: usize) -> String {
    let one = s.replace(['\n', '\r'], " ");
    if one.chars().count() <= max {
        one
    } else {
        let mut t: String = one.chars().take(max).collect();
        t.push('…');
        t
    }
}
