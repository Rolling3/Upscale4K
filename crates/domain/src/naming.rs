//! Output names and the no-overwrite rule.
//!
//! `clip.mkv` becomes `clip_4K.mp4`. A name is never reused silently: an
//! existing file is replaced only when the user chose Replace, and two inputs
//! of one batch that map to the same name get ` (2)`, ` (3)` and so on.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// What to do when the output name already exists in the output folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Keep the existing file and save the new one with a number added.
    KeepBoth,
    /// Replace the existing file.
    Replace,
    /// Leave the existing file and don't upscale this clip.
    Skip,
}

/// Where one input of a batch will be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDecision {
    /// Write to this path; nothing exists there.
    New(PathBuf),
    /// Write over this existing file, as the user asked.
    Replace(PathBuf),
    /// Don't process this input; `existing` stays as it is.
    Skip { existing: PathBuf },
}

/// `<stem>_4K.mp4` for an input path.
pub fn output_file_name(input: &Path) -> OsString {
    let mut name = stem(input);
    name.push("_4K.mp4");
    name
}

/// `<stem>_4K (n).mp4` for an input path.
fn numbered_file_name(input: &Path, n: u32) -> OsString {
    let mut name = stem(input);
    name.push(format!("_4K ({n}).mp4"));
    name
}

fn stem(input: &Path) -> OsString {
    input.file_stem().map(OsStr::to_os_string).unwrap_or_default()
}

/// Key for comparing file names the way NTFS and APFS do by default (case-insensitive).
fn key(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

/// Key for comparing whole paths: case-insensitive, and built from path
/// components so `a/b` and `a\b` match on Windows.
fn path_key(path: &Path) -> String {
    path.components().map(|c| key(c.as_os_str())).collect::<Vec<_>>().join("/")
}

/// The output path an input gets when nothing is in the way.
pub fn default_output(dir: &Path, input: &Path) -> PathBuf {
    dir.join(output_file_name(input))
}

/// Indices of `inputs` whose default output already exists, so the user can
/// be asked once before the batch starts.
pub fn conflicting_inputs(dir: &Path, inputs: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Vec<usize> {
    inputs.iter().enumerate().filter(|(_, input)| exists(&default_output(dir, input))).map(|(index, _)| index).collect()
}

/// A temporary name next to `output` that doesn't exist yet. ffmpeg writes
/// there and the file is renamed when the encode succeeds.
pub fn temp_path(output: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let base = output.file_name().map(OsStr::to_os_string).unwrap_or_default();
    (1u32..)
        .map(|n| {
            let mut name = base.clone();
            if n == 1 {
                name.push(".part");
            } else {
                name.push(format!(".{n}.part"));
            }
            output.with_file_name(name)
        })
        .find(|candidate| !exists(candidate))
        .expect("an unbounded range always yields a free name")
}

/// Hands out output names for one batch and remembers which it gave away.
#[derive(Debug)]
pub struct OutputPlanner {
    dir: PathBuf,
    taken: HashSet<String>,
    inputs: HashSet<String>,
}

impl OutputPlanner {
    /// `batch_inputs` are never replaced, even when the user chose Replace.
    pub fn new(dir: &Path, batch_inputs: &[PathBuf]) -> Self {
        Self {
            dir: dir.to_path_buf(),
            taken: HashSet::new(),
            inputs: batch_inputs.iter().map(|p| path_key(p)).collect(),
        }
    }

    /// Decide the output for `input`, in batch order.
    pub fn decide(&mut self, input: &Path, choice: ConflictChoice, exists: &dyn Fn(&Path) -> bool) -> OutputDecision {
        let base = default_output(&self.dir, input);
        let base_key = key(&output_file_name(input));

        if self.taken.contains(&base_key) {
            return OutputDecision::New(self.next_free(input, exists));
        }
        if !exists(&base) {
            self.taken.insert(base_key);
            return OutputDecision::New(base);
        }
        let is_batch_input = self.inputs.contains(&path_key(&base));
        match choice {
            ConflictChoice::Replace if !is_batch_input => {
                self.taken.insert(base_key);
                OutputDecision::Replace(base)
            }
            ConflictChoice::Replace | ConflictChoice::KeepBoth => OutputDecision::New(self.next_free(input, exists)),
            ConflictChoice::Skip => OutputDecision::Skip { existing: base },
        }
    }

    /// The first numbered name for `input` that is neither on disk nor
    /// already given out, and reserve it. Used when a name turns out to be
    /// taken by the time the encode finishes.
    pub fn next_free(&mut self, input: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
        for n in 2u32.. {
            let name = numbered_file_name(input, n);
            let name_key = key(&name);
            let path = self.dir.join(&name);
            if !self.taken.contains(&name_key) && !exists(&path) {
                self.taken.insert(name_key);
                return path;
            }
        }
        unreachable!("an unbounded range always yields a free name")
    }
}

/// Decide outputs for a whole batch in order.
pub fn assign_outputs(
    dir: &Path,
    inputs: &[(PathBuf, ConflictChoice)],
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<OutputDecision> {
    let all: Vec<PathBuf> = inputs.iter().map(|(p, _)| p.clone()).collect();
    let mut planner = OutputPlanner::new(dir, &all);
    inputs.iter().map(|(input, choice)| planner.decide(input, *choice, exists)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        PathBuf::from("out")
    }

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    fn in_out(name: &str) -> PathBuf {
        dir().join(name)
    }

    fn on_disk(names: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = names.iter().map(|n| in_out(n)).collect();
        move |path: &Path| set.contains(path)
    }

    fn nothing_on_disk(_: &Path) -> bool {
        false
    }

    #[test]
    fn output_name_is_the_stem_plus_4k_as_mp4() {
        assert_eq!(output_file_name(&p("clip.mp4")), "clip_4K.mp4");
        assert_eq!(output_file_name(&p("GAME.MKV")), "GAME_4K.mp4");
        assert_eq!(output_file_name(&p("my.clip.v2.mov")), "my.clip.v2_4K.mp4");
    }

    #[test]
    fn output_name_keeps_non_latin_characters() {
        assert_eq!(output_file_name(&p("لقطة اللعب.mp4")), "لقطة اللعب_4K.mp4");
    }

    #[test]
    fn output_name_ignores_the_input_folder() {
        let input = Path::new("videos").join("raw").join("boss fight.mkv");
        assert_eq!(default_output(&dir(), &input), in_out("boss fight_4K.mp4"));
    }

    #[test]
    fn a_free_name_is_used_as_is() {
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::KeepBoth)], &nothing_on_disk);
        assert_eq!(got, vec![OutputDecision::New(in_out("clip_4K.mp4"))]);
    }

    #[test]
    fn keep_both_adds_a_number_when_the_name_exists() {
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::KeepBoth)], &on_disk(&["clip_4K.mp4"]));
        assert_eq!(got, vec![OutputDecision::New(in_out("clip_4K (2).mp4"))]);
    }

    #[test]
    fn keep_both_skips_numbers_that_are_also_taken() {
        let disk = on_disk(&["clip_4K.mp4", "clip_4K (2).mp4", "clip_4K (3).mp4"]);
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::KeepBoth)], &disk);
        assert_eq!(got, vec![OutputDecision::New(in_out("clip_4K (4).mp4"))]);
    }

    #[test]
    fn replace_targets_the_existing_file_only_when_chosen() {
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::Replace)], &on_disk(&["clip_4K.mp4"]));
        assert_eq!(got, vec![OutputDecision::Replace(in_out("clip_4K.mp4"))]);
    }

    #[test]
    fn replace_on_a_free_name_is_a_plain_new_file() {
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::Replace)], &nothing_on_disk);
        assert_eq!(got, vec![OutputDecision::New(in_out("clip_4K.mp4"))]);
    }

    #[test]
    fn skip_leaves_the_existing_file_alone() {
        let got = assign_outputs(&dir(), &[(p("clip.mp4"), ConflictChoice::Skip)], &on_disk(&["clip_4K.mp4"]));
        assert_eq!(got, vec![OutputDecision::Skip { existing: in_out("clip_4K.mp4") }]);
    }

    #[test]
    fn two_inputs_with_the_same_stem_never_share_an_output() {
        let inputs = [(p("clip.mp4"), ConflictChoice::Skip), (p("clip.mkv"), ConflictChoice::Skip)];
        let got = assign_outputs(&dir(), &inputs, &nothing_on_disk);
        assert_eq!(
            got,
            vec![OutputDecision::New(in_out("clip_4K.mp4")), OutputDecision::New(in_out("clip_4K (2).mp4")),]
        );
    }

    #[test]
    fn names_that_differ_only_in_case_count_as_the_same_name() {
        let inputs = [(p("Clip.mp4"), ConflictChoice::KeepBoth), (p("clip.mov"), ConflictChoice::KeepBoth)];
        let got = assign_outputs(&dir(), &inputs, &nothing_on_disk);
        assert_eq!(
            got,
            vec![OutputDecision::New(in_out("Clip_4K.mp4")), OutputDecision::New(in_out("clip_4K (2).mp4")),]
        );
    }

    #[test]
    fn a_file_this_batch_writes_is_never_replaced_by_a_later_input() {
        let inputs = [(p("clip.mp4"), ConflictChoice::Replace), (p("clip.mkv"), ConflictChoice::Replace)];
        let got = assign_outputs(&dir(), &inputs, &on_disk(&["clip_4K.mp4"]));
        assert_eq!(
            got,
            vec![OutputDecision::Replace(in_out("clip_4K.mp4")), OutputDecision::New(in_out("clip_4K (2).mp4")),]
        );
    }

    #[test]
    fn an_input_of_the_batch_is_never_replaced() {
        // clip.mp4 would replace out/clip_4K.mp4, but that file is queued as an input too.
        let inputs = [(p("clip.mp4"), ConflictChoice::Replace), (in_out("clip_4K.mp4"), ConflictChoice::Replace)];
        let got = assign_outputs(&dir(), &inputs, &on_disk(&["clip_4K.mp4"]));
        assert_eq!(
            got,
            vec![OutputDecision::New(in_out("clip_4K (2).mp4")), OutputDecision::New(in_out("clip_4K_4K.mp4")),]
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_input_of_the_batch_is_recognised_whatever_the_slashes() {
        let inputs = [(p("clip.mp4"), ConflictChoice::Replace), (p("OUT/clip_4K.mp4"), ConflictChoice::Replace)];
        let got = assign_outputs(&dir(), &inputs, &on_disk(&["clip_4K.mp4"]));
        assert_eq!(got[0], OutputDecision::New(in_out("clip_4K (2).mp4")));
    }

    #[test]
    fn conflicts_list_only_inputs_whose_name_exists_on_disk() {
        let inputs = [p("a.mp4"), p("b.mp4"), p("a.mkv"), p("c.mov")];
        let got = conflicting_inputs(&dir(), &inputs, &on_disk(&["a_4K.mp4", "c_4K.mp4"]));
        assert_eq!(got, vec![0, 2, 3]);
    }

    #[test]
    fn next_free_reserves_the_name_it_returns() {
        let mut planner = OutputPlanner::new(&dir(), &[]);
        let first = planner.next_free(&p("clip.mp4"), &on_disk(&["clip_4K.mp4"]));
        let second = planner.next_free(&p("clip.mp4"), &on_disk(&["clip_4K.mp4"]));
        assert_eq!(first, in_out("clip_4K (2).mp4"));
        assert_eq!(second, in_out("clip_4K (3).mp4"));
    }

    #[test]
    fn temp_file_sits_next_to_the_output() {
        assert_eq!(temp_path(&in_out("clip_4K.mp4"), &nothing_on_disk), in_out("clip_4K.mp4.part"));
    }

    #[test]
    fn temp_file_never_reuses_an_existing_name() {
        let disk = on_disk(&["clip_4K.mp4.part", "clip_4K.mp4.2.part"]);
        assert_eq!(temp_path(&in_out("clip_4K.mp4"), &disk), in_out("clip_4K.mp4.3.part"));
    }
}
