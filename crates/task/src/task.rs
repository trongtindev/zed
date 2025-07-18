//! Baseline interface of Tasks in Zed: all tasks in Zed are intended to use those for implementing their own logic.

mod adapter_schema;
mod debug_format;
mod serde_helpers;
mod shell_builder;
pub mod static_source;
mod task_template;
mod vscode_debug_format;
mod vscode_format;

use collections::{HashMap, HashSet, hash_map};
use gpui::SharedString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use std::str::FromStr;

pub use adapter_schema::{AdapterSchema, AdapterSchemas};
pub use debug_format::{
    AttachRequest, BuildTaskDefinition, DebugRequest, DebugScenario, DebugTaskFile, LaunchRequest,
    Request, TcpArgumentsTemplate, ZedDebugConfig,
};
pub use shell_builder::{DEFAULT_REMOTE_SHELL, ShellBuilder};
pub use task_template::{
    DebugArgsRequest, HideStrategy, RevealStrategy, TaskTemplate, TaskTemplates,
    substitute_variables_in_map, substitute_variables_in_str,
};
pub use vscode_debug_format::VsCodeDebugTaskFile;
pub use vscode_format::VsCodeTaskFile;
pub use zed_actions::RevealTarget;

/// Task identifier, unique within the application.
/// Based on it, task reruns and terminal tabs are managed.
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
pub struct TaskId(pub String);

/// Contains all information needed by Zed to spawn a new terminal tab for the given task.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SpawnInTerminal {
    /// Id of the task to use when determining task tab affinity.
    pub id: TaskId,
    /// Full unshortened form of `label` field.
    pub full_label: String,
    /// Human readable name of the terminal tab.
    pub label: String,
    /// Executable command to spawn.
    pub command: Option<String>,
    /// Arguments to the command, potentially unsubstituted,
    /// to let the shell that spawns the command to do the substitution, if needed.
    pub args: Vec<String>,
    /// A human-readable label, containing command and all of its arguments, joined and substituted.
    pub command_label: String,
    /// Current working directory to spawn the command into.
    pub cwd: Option<PathBuf>,
    /// Env overrides for the command, will be appended to the terminal's environment from the settings.
    pub env: HashMap<String, String>,
    /// Whether to use a new terminal tab or reuse the existing one to spawn the process.
    pub use_new_terminal: bool,
    /// Whether to allow multiple instances of the same task to be run, or rather wait for the existing ones to finish.
    pub allow_concurrent_runs: bool,
    /// What to do with the terminal pane and tab, after the command was started.
    pub reveal: RevealStrategy,
    /// Where to show tasks' terminal output.
    pub reveal_target: RevealTarget,
    /// What to do with the terminal pane and tab, after the command had finished.
    pub hide: HideStrategy,
    /// Which shell to use when spawning the task.
    pub shell: Shell,
    /// Whether to show the task summary line in the task output (sucess/failure).
    pub show_summary: bool,
    /// Whether to show the command line in the task output.
    pub show_command: bool,
    /// Whether to show the rerun button in the terminal tab.
    pub show_rerun: bool,
}

impl SpawnInTerminal {
    pub fn to_proto(&self) -> proto::SpawnInTerminal {
        proto::SpawnInTerminal {
            label: self.label.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            env: self
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            cwd: self
                .cwd
                .clone()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        }
    }

    pub fn from_proto(proto: proto::SpawnInTerminal) -> Self {
        Self {
            label: proto.label.clone(),
            command: proto.command.clone(),
            args: proto.args.clone(),
            env: proto.env.into_iter().collect(),
            cwd: proto.cwd.map(PathBuf::from).clone(),
            ..Default::default()
        }
    }
}

/// A final form of the [`TaskTemplate`], that got resolved with a particular [`TaskContext`] and now is ready to spawn the actual task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTask {
    /// A way to distinguish tasks produced by the same template, but different contexts.
    /// NOTE: Resolved tasks may have the same labels, commands and do the same things,
    /// but still may have different ids if the context was different during the resolution.
    /// Since the template has `env` field, for a generic task that may be a bash command,
    /// so it's impossible to determine the id equality without more context in a generic case.
    pub id: TaskId,
    /// A template the task got resolved from.
    original_task: TaskTemplate,
    /// Full, unshortened label of the task after all resolutions are made.
    pub resolved_label: String,
    /// Variables that were substituted during the task template resolution.
    substituted_variables: HashSet<VariableName>,
    /// Further actions that need to take place after the resolved task is spawned,
    /// with all task variables resolved.
    pub resolved: SpawnInTerminal,
}

impl ResolvedTask {
    /// A task template before the resolution.
    pub fn original_task(&self) -> &TaskTemplate {
        &self.original_task
    }

    /// Variables that were substituted during the task template resolution.
    pub fn substituted_variables(&self) -> &HashSet<VariableName> {
        &self.substituted_variables
    }

    /// A human-readable label to display in the UI.
    pub fn display_label(&self) -> &str {
        self.resolved.label.as_str()
    }
}

/// Variables, available for use in [`TaskContext`] when a Zed's [`TaskTemplate`] gets resolved into a [`ResolvedTask`].
/// Name of the variable must be a valid shell variable identifier, which generally means that it is
/// a word  consisting only  of alphanumeric characters and underscores,
/// and beginning with an alphabetic character or an  underscore.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum VariableName {
    /// An absolute path of the currently opened file.
    File,
    /// A path of the currently opened file (relative to worktree root).
    RelativeFile,
    /// A path of the currently opened file's directory (relative to worktree root).
    RelativeDir,
    /// The currently opened filename.
    Filename,
    /// The path to a parent directory of a currently opened file.
    Dirname,
    /// Stem (filename without extension) of the currently opened file.
    Stem,
    /// An absolute path of the currently opened worktree, that contains the file.
    WorktreeRoot,
    /// A symbol text, that contains latest cursor/selection position.
    Symbol,
    /// A row with the latest cursor/selection position.
    Row,
    /// A column with the latest cursor/selection position.
    Column,
    /// Text from the latest selection.
    SelectedText,
    /// The symbol selected by the symbol tagging system, specifically the @run capture in a runnables.scm
    RunnableSymbol,
    /// Custom variable, provided by the plugin or other external source.
    /// Will be printed with `CUSTOM_` prefix to avoid potential conflicts with other variables.
    Custom(Cow<'static, str>),
}

impl VariableName {
    /// Generates a `$VARIABLE`-like string value to be used in templates.
    pub fn template_value(&self) -> String {
        format!("${self}")
    }
    /// Generates a `"$VARIABLE"`-like string, to be used instead of `Self::template_value` when expanded value could contain spaces or special characters.
    pub fn template_value_with_whitespace(&self) -> String {
        format!("\"${self}\"")
    }
}

impl FromStr for VariableName {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let without_prefix = s.strip_prefix(ZED_VARIABLE_NAME_PREFIX).ok_or(())?;
        let value = match without_prefix {
            "FILE" => Self::File,
            "FILENAME" => Self::Filename,
            "RELATIVE_FILE" => Self::RelativeFile,
            "RELATIVE_DIR" => Self::RelativeDir,
            "DIRNAME" => Self::Dirname,
            "STEM" => Self::Stem,
            "WORKTREE_ROOT" => Self::WorktreeRoot,
            "SYMBOL" => Self::Symbol,
            "RUNNABLE_SYMBOL" => Self::RunnableSymbol,
            "SELECTED_TEXT" => Self::SelectedText,
            "ROW" => Self::Row,
            "COLUMN" => Self::Column,
            _ => {
                if let Some(custom_name) =
                    without_prefix.strip_prefix(ZED_CUSTOM_VARIABLE_NAME_PREFIX)
                {
                    Self::Custom(Cow::Owned(custom_name.to_owned()))
                } else {
                    return Err(());
                }
            }
        };
        Ok(value)
    }
}

/// A prefix that all [`VariableName`] variants are prefixed with when used in environment variables and similar template contexts.
pub const ZED_VARIABLE_NAME_PREFIX: &str = "ZED_";
const ZED_CUSTOM_VARIABLE_NAME_PREFIX: &str = "CUSTOM_";

impl std::fmt::Display for VariableName {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::File => write!(f, "{ZED_VARIABLE_NAME_PREFIX}FILE"),
            Self::Filename => write!(f, "{ZED_VARIABLE_NAME_PREFIX}FILENAME"),
            Self::RelativeFile => write!(f, "{ZED_VARIABLE_NAME_PREFIX}RELATIVE_FILE"),
            Self::RelativeDir => write!(f, "{ZED_VARIABLE_NAME_PREFIX}RELATIVE_DIR"),
            Self::Dirname => write!(f, "{ZED_VARIABLE_NAME_PREFIX}DIRNAME"),
            Self::Stem => write!(f, "{ZED_VARIABLE_NAME_PREFIX}STEM"),
            Self::WorktreeRoot => write!(f, "{ZED_VARIABLE_NAME_PREFIX}WORKTREE_ROOT"),
            Self::Symbol => write!(f, "{ZED_VARIABLE_NAME_PREFIX}SYMBOL"),
            Self::Row => write!(f, "{ZED_VARIABLE_NAME_PREFIX}ROW"),
            Self::Column => write!(f, "{ZED_VARIABLE_NAME_PREFIX}COLUMN"),
            Self::SelectedText => write!(f, "{ZED_VARIABLE_NAME_PREFIX}SELECTED_TEXT"),
            Self::RunnableSymbol => write!(f, "{ZED_VARIABLE_NAME_PREFIX}RUNNABLE_SYMBOL"),
            Self::Custom(s) => write!(
                f,
                "{ZED_VARIABLE_NAME_PREFIX}{ZED_CUSTOM_VARIABLE_NAME_PREFIX}{s}"
            ),
        }
    }
}

/// Container for predefined environment variables that describe state of Zed at the time the task was spawned.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TaskVariables(HashMap<VariableName, String>);

impl TaskVariables {
    /// Inserts another variable into the container, overwriting the existing one if it already exists — in this case, the old value is returned.
    pub fn insert(&mut self, variable: VariableName, value: String) -> Option<String> {
        self.0.insert(variable, value)
    }

    /// Extends the container with another one, overwriting the existing variables on collision.
    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }
    /// Get the value associated with given variable name, if there is one.
    pub fn get(&self, key: &VariableName) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }
    /// Clear out variables obtained from tree-sitter queries, which are prefixed with '_' character
    pub fn sweep(&mut self) {
        self.0.retain(|name, _| {
            if let VariableName::Custom(name) = name {
                !name.starts_with('_')
            } else {
                true
            }
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VariableName, &String)> {
        self.0.iter()
    }
}

impl FromIterator<(VariableName, String)> for TaskVariables {
    fn from_iter<T: IntoIterator<Item = (VariableName, String)>>(iter: T) -> Self {
        Self(HashMap::from_iter(iter))
    }
}

impl IntoIterator for TaskVariables {
    type Item = (VariableName, String);

    type IntoIter = hash_map::IntoIter<VariableName, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Keeps track of the file associated with a task and context of tasks execution (i.e. current file or current function).
/// Keeps all Zed-related state inside, used to produce a resolved task out of its template.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskContext {
    /// A path to a directory in which the task should be executed.
    pub cwd: Option<PathBuf>,
    /// Additional environment variables associated with a given task.
    pub task_variables: TaskVariables,
    /// Environment variables obtained when loading the project into Zed.
    /// This is the environment one would get when `cd`ing in a terminal
    /// into the project's root directory.
    pub project_env: HashMap<String, String>,
}

/// This is a new type representing a 'tag' on a 'runnable symbol', typically a test of main() function, found via treesitter.
#[derive(Clone, Debug)]
pub struct RunnableTag(pub SharedString);

/// Shell configuration to open the terminal with.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// Use the system's default terminal configuration in /etc/passwd
    #[default]
    System,
    /// Use a specific program with no arguments.
    Program(String),
    /// Use a specific program with arguments.
    WithArguments {
        /// The program to run.
        program: String,
        /// The arguments to pass to the program.
        args: Vec<String>,
        /// An optional string to override the title of the terminal tab
        title_override: Option<SharedString>,
    },
}

type VsCodeEnvVariable = String;
type ZedEnvVariable = String;

struct EnvVariableReplacer {
    variables: HashMap<VsCodeEnvVariable, ZedEnvVariable>,
}

impl EnvVariableReplacer {
    fn new(variables: HashMap<VsCodeEnvVariable, ZedEnvVariable>) -> Self {
        Self { variables }
    }

    fn replace_value(&self, input: serde_json::Value) -> serde_json::Value {
        match input {
            serde_json::Value::String(s) => serde_json::Value::String(self.replace(&s)),
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(|v| self.replace_value(v)).collect())
            }
            serde_json::Value::Object(obj) => serde_json::Value::Object(
                obj.into_iter()
                    .map(|(k, v)| (self.replace(&k), self.replace_value(v)))
                    .collect(),
            ),
            _ => input,
        }
    }
    // Replaces occurrences of VsCode-specific environment variables with Zed equivalents.
    fn replace(&self, input: &str) -> String {
        shellexpand::env_with_context_no_errors(&input, |var: &str| {
            // Colons denote a default value in case the variable is not set. We want to preserve that default, as otherwise shellexpand will substitute it for us.
            let colon_position = var.find(':').unwrap_or(var.len());
            let (left, right) = var.split_at(colon_position);
            if left == "env" && !right.is_empty() {
                let variable_name = &right[1..];
                return Some(format!("${{{variable_name}}}"));
            }
            let (variable_name, default) = (left, right);
            let append_previous_default = |ret: &mut String| {
                if !default.is_empty() {
                    ret.push_str(default);
                }
            };
            if let Some(substitution) = self.variables.get(variable_name) {
                // Got a VSCode->Zed hit, perform a substitution
                let mut name = format!("${{{substitution}");
                append_previous_default(&mut name);
                name.push('}');
                return Some(name);
            }
            // This is an unknown variable.
            // We should not error out, as they may come from user environment (e.g. $PATH). That means that the variable substitution might not be perfect.
            // If there's a default, we need to return the string verbatim as otherwise shellexpand will apply that default for us.
            if !default.is_empty() {
                return Some(format!("${{{var}}}"));
            }
            // Else we can just return None and that variable will be left as is.
            None
        })
        .into_owned()
    }
}
