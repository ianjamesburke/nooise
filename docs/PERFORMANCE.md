# Live performance

## Prepared states

A bank holds nine numbered states, 1 through 9. Store, recall and cancel
each have a two-key gesture from ordinary browsing and a `/` operation, so the
keyboard path never needs a typed command.

`'` (apostrophe) is go. A digit `1`-`9` queues that state. The digit of the
state already queued cancels the queue. A layer key opens that layer at its
remembered row (`a` Pads, `s` Perc, `d` Bass, `f` Kick, `q` Tonal, `w` Clap,
`e` Arp, `r` the hub), and `j`/`k` put the cursor on Level/Filter of the page
already open. A second `'` queues the previous state: the one that was
current before the most recent landing, so `''` goes back and, repeated,
alternates between two states. With no previous state it says so and
returns to browsing.

`"` (Shift plus the same key) opens the Store leader. A digit `1`-`9` stores the
current song snapshot into that slot at once, replacing whatever is there, with
no confirmation. The message says `Stored state 3` for an empty slot and
`Replaced state 3` for an occupied one. Esc cancels either leader with no
effect. Both leaders act on a key press and need no key-release support, and
both open only from browsing: in the palette, numeric entry, or an editor,
`'` and `"` are ordinary characters or nothing. A terminal may report `"` as the
glyph alone, the glyph with Shift, or `'` with Shift; all three open the Store
leader. A compact overlay lists slots 1-9, each marked `empty` or `filled`,
with `now` on the current state and `next` on the queued one, a title
saying `Go` or `Store state`; go also lists its layer keys and `j`/`k`.

The same operations are reachable through `/`: `/store state 1` to `9`,
`/queue state 1` to `9`, `/queue next state` (the next filled slot in number
order, skipping empty slots and wrapping), `/queue previous state`, and
`/cancel state queue`. There is one store operation and it overwrites. A new
queue replaces the old one; the key header shows the current state and queued
destination, with the bars left (`1 → 2 in 3 bars`, rounded up, so the last bar
reads `in 1 bar`). The change lands at the end of the sounding Phrase, the moment
the chord window wraps to its first chord, and starts the destination at its
saved tempo and state. Queuing requires a running clock; landing exits auto. The
destination starts at beat zero, with a short fade around the boundary. The
landing is read from the pad's own chord cursor every sample, so a Chord Count,
Offset or Length edit made while waiting moves it exactly as it moves the phrase
(edits still wait for the sounding chord to end), and tempo changes never move
it off the wrap. A queue made on the last beat of a phrase lands at that
imminent wrap; one made after the wrap sample waits for the next phrase end.
Under a held or latched Vamp the pad plays one chord, so the end of that chord is
the phrase end. A queued state waits while stopped, shows `waiting`, and after
a restart lands at the first phrase end, a whole phrase from beat zero. The wait
is one phrase: song 1 is 8 chords of 4 bars at 82 BPM, 32 bars or about 94
seconds; the longest possible phrase, 8 chords of 64 bars at 30 BPM, is 512 bars
or 68 minutes.
Physical holds continue across recall until release; held Vamp locks the
destination first chord. The destination supplies the saved q latch and
individual mutes. An empty slot, a stopped clock and a missing previous state
each show a message and leave the current song playing.

Launch with `nooise --set PATH` to save and reopen the bank. Without `--set`,
states last only for this run. The file holds one `number<TAB>song code` line
per filled slot and does not contain recorded audio. A bank that is invalid, or
that still uses the old letter names A, B and C, is refused with a message at
launch (states are now numbered 1 to 9); nothing translates an old file. The
current song keeps playing.

## Push the Harmony (v4)

Hold Space with the thumb and the keys under the fingers push the harmony
from where it is now, never by naming a chord. Taps chain while Space is
down; letting go returns to browsing. Space alone never changes anything.
On a terminal that cannot report key releases, tap Space then one key: one
move.

| key | move | from A minor |
| --- | --- | --- |
| `u` / `j` / `m` | down 2 / 1 / 3 scale steps | F / G / Em |
| `i` / `k` / `,` | up 2 / 1 / 3 scale steps | C / Bdim / Dm |
| `h` / `l` | down / up a fifth | Dm / Em |
| `f` | relative swap (R) | C |
| `d` | major/minor flip (P) | A |
| `s` | leading-tone swap (L) | F |
| `a` | home: the key's tonic | Am |
| `o` | raise the root a half step, to diminished | Bbdim |
| Backspace | undo the last move (16 deep) | |

The step keys sit in columns: on a columnar keyboard the key above or below
`j` and `k` is a bigger step the same way.

`o`, above `l`, is a rising bass: it raises the chord's root a half step and
makes it diminished, the vii° of the chord a step up, and `k` then resolves
it. From a major chord only the root moves (G B D to G# B D), so G, `o`, `k`
plays G, G#dim, Am. From a minor chord the root and the third both rise a
half step (Am to A#dim), which `k` takes to the scale's own chord a step up
(Bdim in A minor). From a diminished chord it climbs again (G#dim to Adim).
The overlay shows it as `♯` on the up-step row; `/push o` and
`/harmony raise root` reach it from the palette. There is no key back to the
authored progression: once pushed, the home row is the instrument (a state
recall or another song is the way out), and undo stops at the first landed
push. Every move and undo is also a `/` operation (`/harmony lift`,
`/push l`, `/harmony down 2 steps`).

By default a move lands on the next beat, so chords are played live and in
time. `/harmony quantize 1`, `2` or `4` makes moves wait for the next 1, 2
or 4 bar line, `/harmony quantize off` lands them at once, and
`/harmony quantize beat` returns to the default. The setting shows in the
overlay, is not saved with songs, resets to `beat` on launch, and re-times a
waiting move when changed. A second move before the line composes from the
queued chord and keeps the line, so a quick `l l` is two fifths up. The pad voices
the new triad from the notes it is sounding, each voice moving to a nearby
chord tone, and changes exactly as an authored chord does: the new chord
fades in over the pad Attack while the old one rings out over Release. The
Bass follows the new root in its own register, and the progression stays on
the sounding slot like a Vamp. The header reads `CHORDS  C → G in 1 bar
PUSH` (the countdown only on a bar grid), and the overlay lists every move
with the chord it would produce. A saved song keeps the pushed chord and any
queued move, which lands on the loaded song's first bar; undo history is not
saved. Holding Space, pushing and letting go keep other physical holds
running, Ctrl+C still quits, and losing focus ends a held push. Smear (glide
a move over a bar) is not built. `proposals/2026-11-11-v4-push-the-harmony.md`
owns the design.

## Slapback preset

`/slapback` applies Delay to the current layer at 40% Amount, 0.25 beats on
both synced channels, and 0% Feedback. It focuses Amount. An existing Delay
is reused and these settings are reapplied, retaining its other settings;
ordinary `/delay` keeps its existing add-or-jump behavior.

## Pad rhythm experiment

Select Pad Level (`/pad.level`), then `/chop`. The ordinary Steps editor opens
on Amount at 0%; raise it, or Shift+Right for a full chop. Rate starts at 0.25
beats per step, Steps at eight. A step at 0% preserves the sound; -100% closes
it at full Amount. Consecutive open steps sustain without retriggering. Arrows
reach rate, offset, count, glide, and step values; `/bypass`, `/resume`, and
`/delete` use the existing lane actions. Ctrl+S saves the ordinary lane.

The chord progression and Slur continue independently. Reverb tails remain
after the level gate. This shortcut also works on other selected controls and
layers; it is not a new Pad envelope mode.

## Hub navigation

The app opens on Master, the hub. Its first rows list the layers in order
(Pads, Perc, Bass, Kick, Tonal, Clap, Arp, Lead). Each row shows the layer's
level and carries `›` to show that Enter opens the layer.
Master's own rows follow. On a layer row, `h`/`l` adjust its level, `m`
mutes that layer, and Enter goes into it; Shift+M mutes Master from any row.

Tab and Shift+Tab step through every layer and then the hub, in Master's
old last place: Pads … Lead, hub, then Pads again. From the hub, Tab enters
Pads and Shift+Tab enters Lead. The hub stays in the cycle because of the
arrow-and-Tab floor (`docs/NORTH_STAR.md`, ADR 0001 invariant 9): a player
who knows only arrows and Tab must still reach every page, and with Esc as
the only way up, Master's own rows (BPM, Tone, Drive) would be lost to them
after leaving the hub. Esc backs out one level at a time: the innermost drill
first, then from the layer's root to the hub with the cursor on that
layer's row. Held gestures stay active while Esc navigates back; at the hub
root with no gesture key held, Esc releases any loaded holds, and otherwise
does nothing. Each layer remembers the row it was left on for the session,
however it is re-entered.

The line above the rows is a breadcrumb: `Master` on the hub, `Master ›
Pads` in a layer, `Master › Pads › Progression` in a drill, `Master › Pads
› Reverb` in a module. A muted page's crumb and hub row read `(M)`, and a
playing chord slot or lane carries `♪`. Its longest paths fit the minimum
frame. Navigation is never saved in a song code.

## Normal browsing gestures

Tap `q` to latch a Kick+Bass group mute; tap again to clear it. Hold for at least 250 ms to mute temporarily, returning to the previous latch state on release. This gate never changes individual layer mutes. It survives in song codes; a physical hold does not. Focus loss or entering an editor releases a hold without changing the latch. `/group mute` toggles the latch, including on terminals without key-release reporting. On such terminals `q` also toggles on each reported press; if the terminal reports autorepeat as indistinguishable presses, use the palette to avoid accidental repeated toggles.

Hold `w` for Submerge, `e` for Rise, `r` for Perc Roll, and `b` for Beat Repeat. Beat Repeat is a quarter-note repeat of the live Master mix: pressing mid-beat plays on dry until the next beat line, then loops the beat the hold began in, locked to the grid, for as long as it is held. The loop length is fixed when it starts, so a tempo change during the hold does not retime it; the next hold uses the new tempo. A hold needs a full beat of live audio behind it, so a loaded hold, or one pressed in the first beat after start, begins looping one beat later. A hold pressed while the transport is stopped waits for it to run. Release is quantized: on key-up the loop keeps playing to the next beat line, then returns to the live mix within 10 ms (so the sound can outlast the key by up to one beat). A release before the loop has engaged cancels at once, and pressing again before the beat line keeps the loop. On a stopped transport release fades at once. After a state landing or transport restart the first loop is a beat later, so it never repeats the song swap. Loop seams and engage are crossfaded. `/repeat` starts the effect without key-release support; Escape releases it. Captured audio is never embedded in song codes. Arrows and Tab remain available while holding. Opening a keyboard-owning editor or losing focus releases physical holds, and a held key must come up before starting again.

Hold or tap `a` in Browsing for an indefinite chord vamp quantized to the start
of the currently playing chord, so whatever chord is sounding stays locked seamlessly
without re-triggering its voices or re-attacking mid-phrase.
Sustaining a hold on `a` keeps the chord locked momentarily under your finger
(`VAMP` beside the chord), and letting go of the key cues release directly
into the next chord on the next chord boundary.
A quick tap (< 250 ms) latches the vamp on hands-free (`VAMP` beside the chord),
allowing you to browse and adjust parameters while the chord holds; a subsequent tap
cues release at the next chord boundary (`VAMP ENDING` beside the chord, and
tapping again before the boundary cancels release and keeps holding).
Vamp toggle is also available in the `/` command palette as `/vamp` (`Toggle vamp`),
and auto-morph toggle is on `Shift+A` or `/auto` (`Toggle auto`). Armed or active
vamp state persists in song codes (`VAMP_RECORD`) and rebases to the loaded song's
beat zero. On terminals lacking key-release support, `a` operates as a latching tap.

The Master effects play over the whole mix after Master's module chain, whatever
page is open. The q gate acts on Kick and Bass before they join the mix. Arrows and Tab remain available while a key is down, and
different gestures can overlap. Pressing during a return catches it at its
current amount. Gestures are gain-staged so a full throw never raises the
master: Bloom and Echo trade dry level for their returns instead of stacking
on top of it.

The footer is two rows: a gesture-activity row above a stable exits/mode-help
row, so a held gesture never crowds out `Esc release · ^Q quit` or the row
below it. Idle, the activity row lists each hold's key and name (`q mute  w submerge  e rise  r roll  b repeat`); held or returning, it switches to a
bold readout of gesture and amount — `↑` means rising, `↓` returning, and `R`
marks a restored hold loaded from a song code. The row below stays a terse
`BROWSE · ? shortcuts   ^Q quit`; pressing `?` opens the full keyboard-shortcut
map (`InteractionMode::Help`), a static overlay covering the breadcrumb/control area
that leaves both footer rows visible beneath it. Esc closes it. Shift+/ is
matched two ways, since terminals disagree on how they report it: the
shifted glyph `?` alone (most terminals — unlike a shifted letter, no SHIFT
modifier accompanies shifted punctuation), or the base key `/` with an
explicit SHIFT modifier (the keyboard-enhancement protocol's report-base-key-
plus-modifier style). A plain, unshifted `/` still opens the palette.

These are temporary effects over the playing song. User module slots remain
intact, automation and auto-morph continue, and edits made during a gesture
survive its return.

Held gestures stay active while Escape backs out of browsing drills and layers;
at the hub root with no gesture key held, Escape releases any restored holds
loaded from a song code.
Opening a keyboard-owning editor, Lead play or the palette releases
gestures. A key held through that transition must be released before
it can start another gesture. Reported focus loss and shutdown also release held
gestures. The exceptions are the performance leaders (go `'`, keep `"`, grab
`g`) and Push the Harmony (Space held): opening one, completing it, or
cancelling it leaves a physical hold running, and a key-up while one is open
releases the hold exactly as in browsing. On a
terminal without key-release support, the latched `q` and `a` toggles are state
rather than holds and likewise survive.

Hold gestures require negotiated key-release support. Without it, q and palette operations remain available; other hold keys remain inactive — the shortcut map
(`?`) still lists them, since they are a capability gap, not a hidden feature.
The runtime never guesses release from a timeout or keyboard repeat.

A saved song carries active gesture amounts and envelope direction, with no
audio buffers. Beat Repeat rebuilds its capture from live audio after loading. Loaded holds resume where they were. A code from a build with
per-layer gestures that carries a gesture aimed at a voice is refused as a
retired per-layer gesture. Press the matching
gesture key to claim a loaded hold and release it, or use Escape to release
all loaded holds.

## Chassis tap tempo (experimental)

`nooise --chassis-tap` explicitly arms tap tempo from the Apple Silicon
MacBook's undocumented SPU accelerometer. The feature is macOS-only and
fails at startup on hardware without that sensor; it does not request
Accessibility, Input Monitoring, or a persistent system permission.

Tap the chassis or desk beside it at the desired quarter-note pulse. The
first two accepted impulses establish a phrase; the third and later set BPM
from the median of the most recent two to four intervals (up to five taps).
Impulses closer than 250 ms are bounce/noise, and a pause longer than two
seconds starts a fresh phrase. A confirmation such as `● TAP 4 · 120 BPM`
appears in the normal footer. The detector runs on its own IOKit run-loop
thread and posts timestamps to the UI loop; it never runs in the audio
callback. Applying BPM uses the ordinary `master.bpm` edit path, so beat
phase is continuous and the result saves with the song while tap history does
not.

The flag is the arm/disarm boundary. Nothing listens by default, and no new
keyboard mode or binding is introduced: arrows and Tab remain available as
usual.

## Clock stop

Shift+P stops the clock and Shift+P again starts it. It is shifted so a stray
keystroke cannot end the song; plain `p` does nothing. It works from browsing
and from an open LFO or envelope editor, on every terminal: a Press edge, no
key-release support needed, and autorepeat never flutters it.

Stopped, the beat holds where it was. No new note, hit, chord, arp, or lane
step fires, and sustaining Pad chords release into their tails. The audio
path keeps running, so note releases, reverb, and delay tails ring out
naturally. Automation and the auto morph wait on the held beat, and gestures
still play into the tails. Played Lead keys still sound. A palette edit
staged for the next bar waits for the clock to start.

Starting returns to bar 1. Every voice grid and phrase position restarts,
and Pads voice the first chord in the selected chord window. Captured knob
loops restart from their first sample, and auto morph restarts its first
endpoint. Pending palette edits land on the first new bar. Live capture
history clears; saved curves and Tonal's evolved notes remain. Existing
release envelopes and effect tails keep ringing. MIDI output sends Start.

While stopped the activity row leads with `■ STOPPED`, whoever owns the
keyboard. Stopping is not an edit: it leaves auto running and the MRU
untouched, like mute.

A song code never carries the stopped state. A stopped song is silence,
not a state worth sharing, so a loaded code always plays.

## Downbeat reset

When the grid stops matching the beat you feel, press `d` on the beat you
feel as one, or use `/downbeat` (also `reset downbeat`, `beat one`, `bar
one`). The nearest beat line becomes beat one of bar one: a press up to half
a beat early waits for the line, and a press up to half a beat late takes
the line just played, so a slightly late tap is never a beat late. The beat
itself never moves: the tempo and the beat phase stay the same, and only the
bar numbering rotates by whole beats. Nothing jumps mid-beat. It works from
browsing on every terminal, as a press edge with no key-release support
needed. Pressing again before a line ahead changes nothing.

A late press applies at once, as though the line just played had been beat
one all along: everything below counts from that line. Hits the old grid
played between the line and the press stand, and nothing is replayed late,
so if the new step 1 was silent on the old grid, the first new hit is the
next one due.

- Harmony: the chord sounding on the line rings on without being struck
  again and starts its count over, so the next chord arrives one full chord
  after the line. No chord is skipped. A chord that was due on the line
  arrives on it, as beat one. A Vamp keeps its chord the same way.
- Rhythm: every grid and step sequencer starts again at step 1 on the line,
  so the Kick's first step hits there. Tonal, Arp and Lead patterns restart,
  and LFO phases follow the new bar.
- Queued state: it still lands at the end of the phrase. The phrase is
  measured from the restarted chord, so the countdown grows by however much
  of that chord had already played.
- Push the Harmony: a waiting move lands on the first line of its quantize
  grid at or after where it was due. A harmony loop treats its sounding chord
  like the progression does: it rings on, starts its count over on the line,
  and the loop carries on from there. `G` still grabs what was heard with
  its real timing.
- Motion loops restart from their first sample on the line. Live edit history
  and Lead phrase history clear, as they do after Shift+P.
- Beat Repeat: a held loop keeps its captured beat and stays on the beat,
  since the line is a beat line. A hold that has not started looping yet
  keeps its count and starts on the line it would have.
- Auto: the morph keeps going, moved back with the restarted chord, so its
  phrases stay on the chords. Pressing `d` doesn't count as an edit, so auto
  keeps running.

While the clock is stopped, `d` only shows a message, because Shift+P already
starts at bar one. Song codes never carry a reset. It only moves the live
clock, and a loaded code always starts at beat zero anyway.

## Master Mode

The Master page has Key (C through B) and Mode (Major, Minor, Dorian,
Phrygian or Mixolydian), always concrete. Launch and loading a song set both
to the progression's own key, so a song sounds as authored until they move;
choosing another progression keeps them, and it plays in them. The
progression is read relative to them: Key transposes it, and Mode re-reads
each diatonic chord on its degree (Harvest's Fmaj7 becomes Fm7 in F minor)
while sus, power and borrowed chords keep their notes, and the Bass under a
re-read chord moves with it. The authored progression is never rewritten.
An authored chord re-voices at once through the pad's Attack/Release, like
a chord edit. Mode is also the home pushes move in: scale steps walk it,
step chords take its qualities, and `a` goes to its tonic chord. A pushed
chord and a loop keep the notes they were given; changing Key or Mode
mid-push leaves the sounding pushed chord alone and only later moves follow
the new home. To hear a loop in another mode, push it there and grab it
again.

Octave (-2 to +2, default 0, `/octave up 1`) moves every pitched voice (Pad,
Bass, Arp, Lead, Tonal) by whole octaves; Perc, Kick and Clap stay. Like a
Key change it lands on each voice's next note, the Pad on its next chord.
It is saved in song codes and lands with the harmony in a morph. The header names the
home (`KEY  Am`, `KEY  A`, `KEY  A Dorian`), and chords in the header and
the Space overlay read as Roman numerals in that home: upper case major,
lower case minor, `°` diminished, a flat for a root off the scale (`bVI`),
extensions kept (`i7`, `IVmaj7`). `/letters` writes letter names instead;
`/roman` returns. `/mode dorian` and `/key d` (or `/key f sharp`) reach
both from the palette. Mode is saved in song codes.

## Harmony loops

`G` then `1`, `2`, `4` or `8` loops the chords heard over the last that-many
bars, with their real timing: authored chords, pushes and loop chords alike,
each with the notes the pad actually sounded. The window ends at the nearest
bar line and the loop starts its first pass there. It is a lane of timed
chords separate from the progression's slots, so a loop can be any of the
four lengths whatever the progression holds, and it changes chords through
the same Attack/Release crossfade as a push. A new grab replaces the loop.
A push while a loop plays ends it when the push lands: you have taken over;
grab again to loop the new material. Backspace (Space held) still undoes
pushes. The header reads `LOOP 4 bars · 2` (the bar of the pass). States and
song codes keep the playing loop and its phase; a loop holds at most 32
chord changes, every change of an 8-bar loop on the default beat grid. The
heard log is live-only and starts over when the clock restarts or a state
lands.

## Go, keep and grab

`'` is go and `"` is keep (see Prepared states). Go also reaches layers: a
layer key opens that layer at the row last active there and hands the
keyboard straight back to browsing, where `h`/`l` adjust it and `j`/`k` move
as always. The layer keys read down the hub's layer rows, so their positions
mirror the layers on screen: `a` Pads, `s` Perc, `d` Bass, `f` Kick, `q`
Tonal, `w` Clap, `e` Arp, and `r` the hub itself. `' j` and `' k` put the
cursor on Level or Filter of the page already open, which is also the way
to Lead's Level. Go changes nothing by itself: it is an address, not an
edit. Digits, `'`, layer keys and `j`/`k` never collide inside the leader.

Volume is the layer's own Level row. `' k` opens the shared Filter detail on
Cutoff. The collapsed Filter row also controls Cutoff on every layer. A newly
added Filter starts at Amount 0% and opens its detail on Amount, so the first
right arrow brings in the filter before a Cutoff sweep. Bass, Kick, Perc and
Clap ship with a filter in slot 1, so `k` lands on the cutoff already in play.
Pads gets one added into its first free slot at a transparent 20 kHz cutoff
and Amount 0%, so arriving is silent. A layer whose chain is full says so and
stays put.

`g` is grab. `g 1`, `g 2`, `g 4` and `g 8` grab Motion from the selected knob:
the last one, two, four, or eight bars. In an LFO or envelope editor, `g`
then a digit grabs the selected field and the editor stays open. `G` then a
digit loops the harmony instead (see Harmony loops below).
A grab belongs to its nearest bar downbeat. When that downbeat is ahead, the
current value holds until it arrives and the loop starts there. When it has
passed, the loop joins immediately at the phase it would already have
reached from that downbeat. Its last value holds through the remainder of
its real one-, two-, four-, or eight-bar phrase. The following downbeat
starts the next pass; continuous movement uses the existing de-clicker and
discrete choices switch at their recorded times. The palette keeps the same
Motion Grab choices: 4, 8, 16, or 32 beats. Searching `grab 4 bars` or
`grab 8 bars` also reaches the matching duration.

Numeric Motion shifts with the knob's base: turning a captured Level, Cutoff,
or Interval keeps the loop running at the same phase around the new value.
The activity row says `shifts with base`. Motion and the other lanes add before
one clamp to the dial's bounds. Grab again to replace the movement; explicit
Bypass, Resume, and Delete keep their lifecycle roles.

Motion Grab also works on discrete controls: Lead pitch cells, Lead Steps,
Pads Trigger (Hold/Stabs), and the other registry switches and choices.
They replay each recorded value at its edit time. A small `↻` on an editor
row means its Motion is active; `○` means bypassed. On a selected row,
`/Motion Bypass`, `/Motion Resume`, and `/Motion Delete` address that row's
Motion. Bare `/bypass`, `/resume`, and `/delete` still address the open LFO or
envelope lane. Numeric editor fields also shift with their base; choices such
as Shape, Trigger, and Steps count replay their recorded values. For these
choices the activity row says `edit bypasses`: a manual edit bypasses the
loop and takes over. Resume waits for the next bar while retaining loop
phase. A saved song restores either kind of loop and its phase. Editor
sliders show the authored base plus a bright marker for the value currently
playing. The selected editor field stays in view at the minimum 46x11 frame.

Every go and grab route acts on a press and needs no key-release support.
Each leader renders a compact centered map; the footer remains visible.
Planned mute (the old `Space m`) is retired: a song code carrying one is
refused by name. Motion Record and a shared lane editor remain deferred.
Lead play retains its own `i` entry and existing bindings.

## Audio smoke

1. Run `cargo run` in a terminal reporting key releases. Tap `q`: Kick and Bass mute; tap again and they return. Mute Kick individually, then use a long `q` hold: releasing must leave Kick muted. Repeat with the group latch already on; the hold must preserve it.
2. Hold `w`, `e`, `r`, and `b` in turn while navigating with arrows and Tab. Beat Repeat should loop the beat you pressed in, in time with the tempo and without clicks at the seam, and on release keep going to the next beat line, then return smoothly. Overlap Beat Repeat with another gesture and check both readouts.
3. Tap `a` and inspect `VAMP` beside the chord. Tap again and inspect `VAMP ENDING` until the next chord boundary. Check both at 46x11 and while browsing another layer.
4. Open the palette while holding a gesture, return to Browse, and keep the key down. It must stay released until a fresh press after key-up. Test `/repeat` and Escape, and `/group mute` on a terminal without key-release support.
5. Save with the q latch and Vamp active, reload the code, and confirm both resume. Save a held Beat Repeat and reload; the audio history must rebuild from live audio instead of containing captured samples.
6. Move to a non-Level Bass row, then press `'`, `d`: it returns to that
   remembered Bass row in Browse with nothing changed, and `h`/`j`/`k`/`l`
   work at once. From Bass press `'`, `j`: the cursor reaches Bass Level.
   Press `'`, `a`, then `'`, `k`: a filter appears on the Pads chain,
   inaudible, cursor on its Cutoff row. Raise Amount in the detail, then `h`
   sweeps Cutoff down. Repeat on Bass and confirm `k` reaches the filter
   already in slot 1 without resetting its cutoff or adding a second one.
   Press `'`, `r`, then `'`, `j` and confirm it reaches Master Level. On
   Lead, confirm `'`, `j` reaches its Level.
7. Move a knob for two bars, then `g`, `2`: the movement loops. Open its LFO
   editor, move a field, `g`, `1`: the field loops and the editor stays open.
   Hold Space and push two chords, let go, then `G`, `1`: the header reads
   `LOOP 1 bar` and the bar's chords, the opening authored one included,
   replay at the moments they were played.
8. Press `?` from Browsing: the shortcut map should open over the
   breadcrumb and control rows, leaving both footer rows visible beneath
   it. Esc returns to Browsing.
9. On Pads with Reverb loaded and Kick audible, press Shift+P: no new kick or
   chord arrives, the chord and reverb fade out naturally rather than
   cutting, and the activity row shows `■ STOPPED`. Hold `w` during the
   tail and hear it darken. Press Shift+P again: the grid restarts at bar 1 and
   the first chord in the selected window swells back in.
10. Launch: the hub shows `Master` above one `›` row per layer. Press `m` on
   Kick: its row and nothing else reads `(M)`. Enter Bass, move down two
   rows, Tab to Kick and Esc: the cursor is on the hub's Kick row. Enter
   Bass again: the cursor is back where it was left. Shift+Tab to Lead, then
   Tab: the hub is back, with BPM reachable by arrows.
11. With `--set PATH`, press `"` then `1`, `2`, `3` in turn after changing a knob
   each time: the message reads `Stored state N`. Press `'` then `2`: the
   header counts down the bars (`in N bars`) and the state lands when the chord
   progression loops back to its first chord. Press `'` then
   `2` again before the bar: the queue cancels. After a landing, press `'` `'`:
   the previous state is queued. Press `"` then `2`: `Replaced state 2`, with
   no prompt. Press `'` then `9`, an empty slot: a message, and the song plays on.
   Hold `w` and press `'` then `3`: the Submerge stays held through the landing.
   Quit and relaunch with the same path: the filled slots show `filled` in the
   `'` overlay and recall as stored. Check both overlays at 46x11.
12. Edit the set file to start a line with `A`: relaunch shows the refusal at
   launch and the song plays.
13. With Kick audible, tap `d` just before a beat that is not the bar's first:
   that beat becomes beat one, with no stumble in the pulse. The chord keeps
   ringing with no re-attack, and the ribbon restarts that chord's first
   measure. Queue a state, tap `d` mid-phrase: the `in N bars` countdown
   grows, and the landing falls on the new grid's phrase end.

13. On song 1 (A minor), hold Space and tap `f`: C fades in at once over
   the pad Attack, exactly like the progression's own chord changes, with
   the least voice movement, and the Bass steps up a minor third. Still
   holding, tap `u` then `,`: two separate changes, a third down then a
   fourth up. Let Space go: browsing keys work again. `/harmony quantize 1`,
   then hold Space and tap `l` `l` before the bar: the header counts down and
   it lands on B diminished. Hold Space and press Backspace: back to E minor
   on the next bar. Hold `w` through all of it: Submerge stays held. Save
   with a chord pushed, reload: it sounds at once.

Acceptance requires rendered-audio checks as well as input replay and footer
checks. The minimum supported frame is 46x11 — the two-row footer costs the
control-row area one line versus the former 46x10.
