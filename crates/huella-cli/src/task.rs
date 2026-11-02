//! `huella task`: revisar por tarea (lo que se va a mergear), no por sesión.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use huella_engine::task::{self, RepoSync};
use huella_store::{Store, TaskRepo};

#[derive(Subcommand)]
pub enum TaskCommand {
    /// Crea la tarea (o cambia su descripción) y busca sus ramas: las que
    /// llevan la clave en el nombre, en los repositorios de tus sesiones y en
    /// la carpeta actual.
    New {
        /// Clave de la tarea (p. ej. DENMS-1234).
        key: String,
        #[arg(long)]
        title: Option<String>,
        /// Lo que pide la tarea, pegado tal cual.
        #[arg(long, conflicts_with = "description_file")]
        description: Option<String>,
        /// Fichero con la descripción; `-` la lee de la entrada estándar.
        #[arg(long)]
        description_file: Option<PathBuf>,
        /// Rama a la que se mergean todas (p. ej. la del sprint).
        #[arg(long)]
        target: Option<String>,
        /// Repositorios donde buscar además de los conocidos.
        #[arg(long = "repo")]
        repos: Vec<PathBuf>,
    },
    /// Tareas guardadas.
    List,
    /// Repositorios de la tarea, cómo están respecto al remoto y qué avisos hay.
    Show {
        key: String,
        /// No traer el remoto (más rápido, puede estar desactualizado).
        #[arg(long)]
        no_fetch: bool,
    },
    /// Fija la rama a la que se mergea: en todos los repositorios o en uno.
    Target {
        key: String,
        target: String,
        /// Solo en este repositorio (ruta o nombre de la carpeta).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Añade un repositorio a mano.
    Add {
        key: String,
        repo: PathBuf,
        /// Rama de la tarea (por defecto, la que tiene la copia de trabajo).
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
    /// Quita un repositorio de la tarea.
    Remove {
        key: String,
        /// Ruta o nombre de la carpeta.
        repo: String,
    },
    /// Lo que se va a revisar: ficheros por repositorio.
    Review {
        key: String,
        #[arg(long)]
        no_fetch: bool,
    },
    /// Borra la tarea (no toca las ramas).
    Delete { key: String },
}

pub fn command(store: &Store, sources: &huella_engine::Sources, action: TaskCommand) -> Result<()> {
    match action {
        TaskCommand::New { key, title, description, description_file, target, repos } => {
            let key = key.to_ascii_uppercase();
            let description = match (description, description_file) {
                (Some(text), _) => text,
                (None, Some(path)) => read_description(&path)?,
                (None, None) => store.task(&key)?.map(|task| task.description).unwrap_or_default(),
            };
            store.save_task(&key, title.as_deref(), &description)?;
            let mut extra = repos;
            extra.push(std::env::current_dir()?);
            let added = task::discover(store, &key, &extra)?;
            if let Some(target) = &target {
                set_target(store, &key, target, None)?;
            }
            crate::emit(&format!("Tarea {key}: {} repositorios nuevos.\n", added.len()));
            crate::emit(&render_show(store, &key, false)?);
            Ok(())
        }
        TaskCommand::List => {
            let mut out = String::new();
            for task in store.tasks()? {
                let repos = store.task_repos(&task.key)?;
                let title = task.title.unwrap_or_else(|| first_line(&task.description));
                let _ = writeln!(out, "{}  {}  · {} repositorios", task.key, title, repos.len());
            }
            crate::emit(if out.is_empty() { "No hay tareas: créalas con huella task new <CLAVE>.\n" } else { &out });
            Ok(())
        }
        TaskCommand::Show { key, no_fetch } => {
            crate::emit(&render_show(store, &key.to_ascii_uppercase(), !no_fetch)?);
            Ok(())
        }
        TaskCommand::Target { key, target, repo } => {
            let changed = set_target(store, &key.to_ascii_uppercase(), &target, repo.as_deref())?;
            crate::emit(&format!("Destino {target} en {changed} repositorios.\n"));
            Ok(())
        }
        TaskCommand::Add { key, repo, branch, target } => {
            let key = key.to_ascii_uppercase();
            let root = huella_engine::task::repo_root(&repo)
                .with_context(|| format!("{} no es un repositorio de git", repo.display()))?;
            let branch = match branch {
                Some(branch) => branch,
                None => huella_engine::task::checked_out(&root).context("indica la rama con --branch")?,
            };
            store.task(&key)?.with_context(|| format!("no hay ninguna tarea {key}: créala con huella task new"))?;
            store.set_task_repo(&key, &TaskRepo { repo: root.display().to_string(), branch, target })?;
            crate::emit(&render_show(store, &key, false)?);
            Ok(())
        }
        TaskCommand::Remove { key, repo } => {
            let key = key.to_ascii_uppercase();
            let found = find_repo(store, &key, &repo)?;
            store.remove_task_repo(&key, &found.repo)?;
            crate::emit(&format!("Quitado {}.\n", found.repo));
            Ok(())
        }
        TaskCommand::Review { key, no_fetch } => {
            let key = key.to_ascii_uppercase();
            let review = task::task_review(store, sources, &key, !no_fetch)?;
            crate::emit(&render_review(&review));
            Ok(())
        }
        TaskCommand::Delete { key } => {
            let key = key.to_ascii_uppercase();
            if !store.delete_task(&key)? {
                bail!("no hay ninguna tarea {key}");
            }
            crate::emit(&format!("Tarea {key} borrada.\n"));
            Ok(())
        }
    }
}

fn read_description(path: &Path) -> Result<String> {
    if path == Path::new("-") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("no se pudo leer {}", path.display()))
    }
}

/// Fija el destino en un repositorio o en todos. Devuelve en cuántos.
fn set_target(store: &Store, key: &str, target: &str, repo: Option<&str>) -> Result<usize> {
    let repos = match repo {
        Some(repo) => vec![find_repo(store, key, repo)?],
        None => store.task_repos(key)?,
    };
    for found in &repos {
        store.set_task_repo(key, &TaskRepo { target: Some(target.to_owned()), ..found.clone() })?;
    }
    Ok(repos.len())
}

/// Un repositorio de la tarea por su ruta o por el nombre de su carpeta.
fn find_repo(store: &Store, key: &str, wanted: &str) -> Result<TaskRepo> {
    let repos = store.task_repos(key)?;
    let root = huella_engine::task::repo_root(Path::new(wanted)).map(|root| root.display().to_string());
    let matches: Vec<TaskRepo> = repos
        .into_iter()
        .filter(|repo| {
            Some(&repo.repo) == root.as_ref() || Path::new(&repo.repo).file_name().is_some_and(|name| name == wanted)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("la tarea {key} no tiene el repositorio {wanted}"),
        _ => bail!("{wanted} es ambiguo: indica la ruta"),
    }
}

fn render_show(store: &Store, key: &str, fetch: bool) -> Result<String> {
    let task = store.task(key)?.with_context(|| format!("no hay ninguna tarea {key}"))?;
    let repos = store.task_repos(key)?;
    let mut out = String::new();
    let title = task.title.clone().unwrap_or_else(|| first_line(&task.description));
    let _ = writeln!(out, "{key} · {}", if title.is_empty() { "(sin descripción)" } else { &title });
    if repos.is_empty() {
        let _ = writeln!(out, "  Sin repositorios: añádelos con huella task add {key} <carpeta>.");
    }
    let preferred = repos.iter().find_map(|repo| repo.target.clone());
    for repo in &repos {
        let state = task::sync(Path::new(&repo.repo), &repo.branch, repo.target.as_deref(), fetch);
        render_repo(&mut out, &state);
        if repo.target.is_none()
            && let Ok(candidates) = task::suggest_targets(Path::new(&repo.repo), &repo.branch, preferred.as_deref())
            && !candidates.is_empty()
        {
            let names: Vec<&str> = candidates.iter().map(|candidate| candidate.name.as_str()).collect();
            let _ = writeln!(
                out,
                "    destino probable: {} (fíjalo con huella task target {key} <rama>)",
                names.join(", ")
            );
        }
    }
    Ok(out)
}

fn render_repo(out: &mut String, state: &RepoSync) {
    let name = Path::new(&state.repo).file_name().map_or(state.repo.clone(), |n| n.to_string_lossy().into_owned());
    let target = state.target.as_deref().unwrap_or("¿?");
    let _ = writeln!(out, "  {name}: {} → {target} · {} commits", state.branch, state.commits);
    if let Some(error) = &state.error {
        let _ = writeln!(out, "    ✗ {error}");
    }
    for warning in state.warnings() {
        let _ = writeln!(out, "    ! {warning}");
    }
}

fn render_review(review: &task::TaskReview) -> String {
    let mut out = String::new();
    for state in &review.repos {
        render_repo(&mut out, state);
        let prefix = format!("{}/", state.repo);
        for file in review.review.files.iter().filter(|file| file.path.starts_with(&prefix)) {
            let (added, removed) =
                file.hunks.iter().fold((0, 0), |(a, r), hunk| (a + hunk.added(), r + hunk.removed()));
            let _ = writeln!(out, "    {}  +{added} −{removed}", &file.path[prefix.len()..]);
        }
    }
    for path in &review.skipped {
        let _ = writeln!(out, "  (sin revisar línea a línea: {path})");
    }
    out
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().chars().take(80).collect()
}
