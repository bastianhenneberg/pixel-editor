# Pixel animation: walk, idle, bob, attached items, mirroring

Reference size: small sprites (about 48x48), 8-frame walk, top-down / 3⁄4 view. Markers as in `fundamentals`.

## Walk cycle: four positions per step [consensus]
1. **Contact:** front foot touches down, arms/legs at the swing extreme.
2. **Down:** weight lands, knee gives – **lowest point**.
3. **Passing:** supporting leg straight, free leg passes under the body, limbs close to the body.
4. **Up:** push-off – **highest point**. Then contact of the other side.
- Sources disagree: Slynyrd 50 puts contact low and passing high; Williams/Saint11 down low and up high. Both work **[style]**; recommended: Williams/Saint11.
- **The passing pose defines the character of the walk** (lower = more cartoony).
- **Arms opposite to the legs.**
- **The supporting foot moves back every frame**, or it slides. Check ground contact with `render_animation` and `track` on the feet.

## Frame count [consensus]
- 8 frames = best compromise (Slynyrd), 6 still fluid, 4 only key poses.
- When cutting: keep contact and passing; drop down/recoil first (8→6), then up (6→4). Fewer frames → longer durations.

## Timing in numbers
- Slynyrd: run 8 × 80 ms or 4 × 160 ms.
- Pixnote: 8–12 FPS common; idle ~6 FPS (≈167 ms), walk ~8 FPS (125 ms), attack 10–12 FPS.
- Williams: one step on 12 drawings (½ s) = brisk, 8 = cartoony / slow run, 16 = stroll.
- About 95 ms per frame (8 frames) feels brisk; if it looks hurried try 100–125 ms. Game speed must match the step length.
- **Uneven frame durations are allowed and good** (contact/down longer, passing shorter).

## Bob [consensus]
- Without up and down the figure floats.
- 1–2 px on small sprites; **2 bob cycles per 8-frame loop** (one per step).
- Triangle wave, not sine.
- **Walking away (back view) has less vertical motion** than walking towards the camera.
- Lowering/raising the head 1 frame after the torso = overlap for free.

## Top-down walk (Saint11) [consensus]
- Start from the idle frame. Overlap: feet behind legs, legs behind torso, torso behind head.
- Side view like a platformer cycle; strong contrast points on hands and feet.

## Attached items [consensus/practice]
- **Rigid parts (sword, scabbard, helmet) keep their shape and pixel pattern** and only move as a whole, with the bob of the body part they hang on (belt → hip, back → torso).
- **Loose parts (straps, tassels, cape, strands) lag 1 frame** and travel on arcs.
- **[practice] Keep weapon/cape on their own layer**, so they are moved pixel-identically with `move_region` / `copy_region` instead of being redrawn.

## Asymmetry and mirroring [consensus]
- Common: draw 5 directions, mirror side and diagonals – **only works for a symmetric figure**.
- Mirrored, the sword changes hand/side and directional light flips.
- Solutions by effort: (1) accept it (Slynyrd: "acceptable", noticeable mainly when turning on the spot) · (2) mirror the body, re-place the item on its own layer (`mirror_document`, then fix the weapon layer) · (3) separate animations per direction · (4) only 4 directions.
- Match neighbouring directions: head bob, shoulder pivot, maximum swing, limb thickness, **position of the equipment** (or it jumps when the direction changes).

## Subpixel (Saint11) [consensus]
- Change an edge color step by step through ramp tones → the edge seems to move by fractions of a pixel.
- Move one part first, then the rest; move inner details before/after the silhouette.
- **"Mostly move light, avoid moving shapes":** highlights travel, the silhouette stays.
- Shifting the break points of lines = very fine motion.
- Too many in-between colors make it muddy; use only existing palette tones (`shade` along the ramps).

## Idle [consensus, amount of bounce is style]
- 2 frames: 1 px down (bend knees, open arms) / 1 px up. Mass stays constant.
- 3 frames: a middle frame slows the sinking, face delayed.
- 4–6 frames: secondary motion (hair swinging after), subpixel.
- Pure vertical motion is boring; add something horizontal. Hold extremes. Tempo ~167 ms or slower, change only a few pixels per frame.
- Personality through special animations (blinking, looking around).

## Smear [consensus]
- For attacks: 1 smear frame (crescent, arc, streak) plus stretch; bright colors bleed over dark ones. Usually unnecessary for walks.

## Avoid flicker [practice + Slynyrd]
- **Static parts stay pixel-identical** – a single stray pixel is visible at once in the loop. Linked cels (`link_cel`, `add_frame` with `linked`) or `copy_region` instead of redrawing.
- Move highlights calmly and rhythmically, move clusters as a whole, no dithering on moving areas.
- Check: onion skin and difference row (`render_animation`), then the loop **at 1x and in real time** (editor preview or GIF export).

## Sources
- Saint11: https://saint11.art/img/pixel-tutorials/ (Walk.gif, TopDownWalkCycle.gif, Subpixel.gif, characterIdle.gif, MotionBlur.gif, Squash.gif)
- Slynyrd Pixelblog 8 (intro to animation): https://www.slynyrd.com/blog/2018/8/19/pixelblog-8-intro-to-animation
- Slynyrd Pixelblog 22, 50, 55 (see `fundamentals` and `animation-principles`)
- https://pixnote.net/en/learn/animation
- Williams reading notes: https://s184753.blogspot.com/2019/01/more-research-animators-survival-kit.html

## Gaps
Not reachable at research time: blog.studiominiboss.com, OpenGameArt "Lines and Curves", Pixel Logic (paid). Pixel boiling, onion skinning and foot sliding without a primary source read ([practice]).
