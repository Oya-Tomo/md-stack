//! TeX → SVG rendering with MathJax running in an embedded QuickJS engine.
//!
//! QuickJS contexts are not `Send`, so the engine lives on a dedicated thread and
//! requests are passed to it over a channel.

use std::sync::mpsc;
use std::thread;

use anyhow::{Context as _, Result, anyhow};
use rquickjs::{Context, Function, Runtime};
use serde::Deserialize;
use tokio::sync::oneshot;

/// MathJax bundle built from `mathjax/entry.js` (see `mathjax/package.json`).
const MATHJAX_JS: &str = include_str!("../assets/mathjax.js");

#[derive(Debug, Clone, Deserialize)]
pub struct Rendered {
    pub svg: String,
    pub width_ex: f32,
    pub height_ex: f32,
    pub depth_ex: f32,
}

/// One expression's outcome; `Err` carries MathJax's error message.
pub type Outcome = std::result::Result<Rendered, String>;

struct Job {
    items: Vec<(String, bool)>,
    reply: oneshot::Sender<Result<Vec<Outcome>>>,
}

pub struct MathRenderer {
    jobs: mpsc::Sender<Job>,
}

impl MathRenderer {
    /// Starts the engine thread. MathJax is loaded in the background, so this returns at once.
    pub fn spawn() -> Self {
        let (jobs, receiver) = mpsc::channel::<Job>();
        thread::spawn(move || {
            let engine = Engine::new();
            for job in receiver {
                let result = match &engine {
                    Ok(engine) => engine.render(&job.items),
                    Err(e) => Err(anyhow!("MathJax failed to load: {e:#}")),
                };
                let _ = job.reply.send(result);
            }
        });
        Self { jobs }
    }

    /// Renders `(tex, display)` pairs as one batch; macros defined in the batch stay local to it.
    pub async fn render(&self, items: Vec<(String, bool)>) -> Result<Vec<Outcome>> {
        let (reply, response) = oneshot::channel();
        self.jobs
            .send(Job { items, reply })
            .map_err(|_| anyhow!("math engine thread has stopped"))?;
        response.await.context("math engine thread has stopped")?
    }
}

/// A QuickJS context with MathJax loaded. The context keeps its runtime alive.
struct Engine {
    context: Context,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum JsOutcome {
    Ok(Rendered),
    Err(String),
}

impl Engine {
    fn new() -> Result<Self> {
        let runtime = Runtime::new()?;
        // MathJax's parser recurses deeply on nested input.
        runtime.set_max_stack_size(16 * 1024 * 1024);
        let context = Context::full(&runtime)?;
        context.with(|ctx| ctx.eval::<(), _>(MATHJAX_JS))?;
        Ok(Self { context })
    }

    fn render(&self, items: &[(String, bool)]) -> Result<Vec<Outcome>> {
        let (sources, displays): (Vec<String>, Vec<bool>) = items.iter().cloned().unzip();
        let json: String = self.context.with(|ctx| {
            let render: Function = ctx.globals().get("mdStackRenderTex")?;
            render.call((sources, displays))
        })?;
        let outcomes: Vec<JsOutcome> = serde_json::from_str(&json)?;
        Ok(outcomes
            .into_iter()
            .map(|outcome| match outcome {
                JsOutcome::Ok(rendered) => Ok(rendered),
                JsOutcome::Err(message) => Err(message),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(items: &[(&str, bool)]) -> Vec<Outcome> {
        let renderer = MathRenderer::spawn();
        let items = items.iter().map(|&(t, d)| (t.to_owned(), d)).collect();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(renderer.render(items))
            .unwrap()
    }

    #[test]
    fn renders_and_reports_errors() {
        let out = render(&[
            (r"x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}", true),
            (r"\frac{a}{b", false),
            (r"\foo", false),
        ]);
        let ok = out[0].as_ref().unwrap();
        assert!(ok.svg.starts_with("<svg"));
        assert!(ok.width_ex > 0.0 && ok.height_ex > 0.0);
        assert_eq!(out[1].as_ref().unwrap_err(), "Missing close brace");
        assert_eq!(
            out[2].as_ref().unwrap_err(),
            r"Undefined control sequence \foo"
        );
    }

    #[test]
    fn macros_do_not_leak_between_batches() {
        let renderer = MathRenderer::spawn();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let first = rt
            .block_on(renderer.render(vec![
                (r"\newcommand{\R}{\mathbb{R}}\R".into(), false),
                (r"\R".into(), false),
            ]))
            .unwrap();
        assert!(first.iter().all(Result::is_ok));
        let second = rt
            .block_on(renderer.render(vec![(r"\R".into(), false)]))
            .unwrap();
        assert!(second[0].is_err());
    }
}
