# Workflow: how to work on pixel art with this editor

Entry point for AI assistants. Read once per session before new characters, new facing directions or new animations, so known lessons are applied early instead of repeating the same design mistake. The craft rules live in the other topics: `fundamentals`, `animation-principles`, `pixel-animation`, `checklist`.

## Artistic role
- Work as a pixel artist and animator: decide deliberately on silhouette, volume, perspective, readability, palette and motion.
- A technically valid document is no proof of artistic quality. Automation serves the drawing; it never replaces looking at the result.
- Judge your own results honestly. Never report success just because a tool call succeeded.

## Before drawing
1. Look at the approved originals and note the character's traits: silhouette, proportions, face, equipment, palette, level of detail.
2. Pin down the target: canvas size, game camera, facing/movement direction, display size, outline weight, transparency, animation needs. Ask about missing essentials; read the project rules (`.pxe-instructions.md`, project knowledge).
3. Choose the way of working: a small concrete pixel fix directly in the document; a larger design change first as a single pose / draft that is reviewed. If a mark or request is unclear, read the area (`get_user_view`, `get_rows`) instead of guessing.

## Creating or importing templates
- For new characters or difficult perspectives an image model fed with the original sprites can help with design exploration. Imported templates are equally valid inputs.
- When generating, state explicitly: flat 2D game art, exact target canvas, limited palette, camera angle, facing direction, character identity, a consistent pixel grid, clear outlines and the required pose. Avoid 3D render lighting, arbitrary extra details and style drift when the game uses flat 2D pixel art.
- A high-resolution PNG is not a better sprite. Model output can miss the grid, the palette and the detail budget. Mark high-res drafts as references; never shrink them unchecked or hand them over as finished sprites.
- Store each variant separately. Compare several side by side, let the user choose, keep origin and prompt with each. Protect the approved state; generating is not approval.
- A reference shown on a small canvas (`set_reference`) can show far more detail when zoomed than the real pixels will. Say so openly before animating.
- Native redraw: silhouette and color areas first, then face and equipment, last sparse highlights and outline fixes. Check at real game size AND zoomed.

## Character identity and perspective
- Keeping the identity does not mean copying every pixel. Traits must be redrawn so they make sense from the new angle.
- Scaling, shearing or shifting an old sprite does not create a real new view. Use such helpers only as groundwork and fix their artifacts deliberately.
- Head, face, body, hands, feet and carried items must all follow the same camera and orientation.
- Especially in diagonal walk cycles check toe direction, visible tops/soles, hip line, body sides and overlaps. Making the body narrower and keeping the old side-view shoes is not enough.
- Flat 2D art can still have spatially consistent perspective; it does not need a 3D render look.

## Transferring an approved template
- The approved template is the binding character reference. Do not silently replace the palette, change proportions or reduce eyes to lines.
- If the user names an existing document palette as the target, use exactly that palette (keys, values, names); do not re-extract colors from a PNG. Same keys in different documents do not mean same colors: remap all pixel values and check the resulting colors visually.
- Clarity comes from deliberately shaped, connected color clusters and readable outlines. Clean up blend colors and fringes created by importing; keep character-defining highlights, eyes and small equipment parts. Fewer colors alone prove nothing, and deliberate anti-aliasing is not wrong in general.
- After an import check the character-defining clusters (eye width, highlights, mouth, helmet, outline). Keep a body drawing that does not change between poses on its own layer with linked cels; steps and arms may change independently.
- If the target format is fixed, keep it. A larger resolution is no substitute for a successful native version.

## Pose first, then animation
- First work out one convincing single pose and have it judged. Only then derive the cycle.
- Choose the frame count from what the motion needs. An existing frame count is no obligation; more frames do not repair a wrong pose.
- Plan contact and passing poses, supporting leg, weight shift, foot path in the direction of travel and counter-swing of the arms. Expect a fixed ground line only where a foot actually holds contact; lifted feet may vary.
- Check the loop transition, silhouette jumps, floating or sliding feet and direction changes (`render_animation` with `track`).

## Visual control
- Look at the actual document state. Document or mark queries do not replace looking at the image (`render_preview`, `render_animation`).
- Use zoomed views and real game size. A filmstrip helps comparing; check motion additionally as a sequence in time (the running preview in the editor, or a GIF export).
- Claim only what you actually checked. A saved PNG is not a checked animation; a single screenshot is not a watched loop.
- Ask the honest quality question: Is it recognisably the same character? Does the direction read without explanation? Do all body parts agree? Is the pose readable, without distortions or stray pixels?

## Collaboration and safe drafts
- Keep originals safe (`snapshot` before larger changes); keep drafts separate from the production export (`save_as` a draft file). Never leave a rejected variant as the current approved state.
- Work precisely by document, frame, layer and region. Respect unsaved changes of the user.
- For targeted feedback change only the part named (the tip of the grip, not the whole grip). Use the user's marks (`get_user_view`) and remove them afterwards (`clear_marks`).
- Record accepted and rejected examples with the reason. Distinguish observation, user preference, confirmed rule and open hypothesis. Revise contradicting rules instead of piling them up. Record confirmed lessons with `add_learning`.

## Reusable prompt frame (image models)
```
Asset / purpose: …
Target grid and display size: …
Original references and essential character traits: …
2D style, palette, outlines, level of detail: …
Camera and shared body/foot orientation: …
Single pose or motion: …
Output: separate transparent template, not yet an approved game sprite.
Avoid: …
```
For a generated walk-cycle sheet give the original PNG as reference, a clear sheet grid and the native cell size (a 4x2 sheet of eight 48x48 frames is logically 192x96 px), and describe contact/down/passing poses, separate foot paths and counter-swing. Check the output anyway; the prompt guarantees no correct grid.

## Experience, not a guarantee
Algorithmically reshaped diagonal drafts have lost character identity or kept wrongly facing shoes; image-model templates made from the original sprites helped design exploration but came out too three-dimensional and off-grid. Templates can improve design finding; neither an image model nor a stronger language model guarantees correct pixel art.
