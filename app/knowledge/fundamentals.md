# Fundamentals: lines, clusters, light, color, outlines

Markers: **[consensus]** = several sources agree · **[style]** = taste or project decision · **[practice]** = common craft without a primary source read. A palette or rule set by the user always takes precedence; these rules explain, they do not replace it.

## Lines and jaggies
- **[consensus] Equal step lengths.** Clean slopes: 1:0, 1:1, 1:2, 2:1, 1:3 … always whole numbers. Irregular sequences (1-3-1) read as a kink. In-between slopes by regular alternation (2-1-2-1).
- **[consensus] Curves:** run lengths change monotonically (4-3-2-1-1-2-3-4), never jumping (4-1-3).
- **[consensus] Remove doubles:** 1-px lines without L-corners and doubled step pixels.
- **Order:** block the shape → even out the steps → remove doubles → remove orphan pixels.
- **Exception:** deliberate corners for sharp things (blades, feathers, arrow heads).

## Clusters, noise, banding
- **[consensus] No orphan pixels.** Every pixel belongs to a cluster of its color (exception: deliberate AA).
- **[consensus] Cluster shapes are connected:** changing one changes its neighbour's shape too.
- **[consensus] No banding:** neighbouring color bands must not end on the same x/y coordinate (not as a staircase, at 45° or parallel to the outline either = "hugging"). Remedies: offset the edges, vary band widths, use fewer steps.

## Light and form
- **[consensus] One light source for everything** (top-down: usually top-left or top-front).
- **[consensus] No pillow shading** (dark rings following the silhouette). Shade the 3D form: bottom/back dark, faces towards the light bright. About 3 steps per material: base, shadow, deep shadow.
- **Checks:** squint (a few large light/dark clusters?), desaturate (does the value structure hold in grey?), flip horizontally (reveals proportion errors; `transform` op `flip_h` on a copy, or the editor's mirror view).

## Color ramps
- **[consensus] Hue shift:** shift the hue by up to ~20° per step, lighter = warmer, darker = cooler. Straight ramps look flat.
- **[consensus] Brightness rises evenly, saturation peaks in the middle**, never 0 % or 100 %. Bright + highly saturated = garish.
- **[consensus] Small palettes** give cohesion. With ~20 colors: about 3 steps per material plus shared neutrals.
- In the editor: keep `set_ramps` and `shade` in line with this logic.

## Outlines
- **[style] Continuous dark outer outline:** reads on any background but costs space.
- **[consensus] Inner outlines lighter than the outer outline:** the darkest tone of the material instead of black. The outer outline separates from the background, inner lines separate forms.
- **[style] Sel-out** (colored outline, lightened at lit edges) is debated; decide per project.
- **[consensus] Never anti-alias the outer edge towards the background** when the background changes (grass, paths).
- Editor: `outline` (mode outside/inside, connectivity 4/8, only_transparent).

## Anti-aliasing and dithering
- **[consensus] AA only inside and sparingly**, at step corners; longer segments → longer AA runs; no AA parallel to the line (banding).
- **[consensus] Dither sparingly**, only on large areas / rough textures, not on small characters; no random dithering; if more than half an area is dithered, use a palette color instead. Editor: `fill`/`draw_rect` with `dither`.
- **[practice] No dithering on moving parts** (it flickers).

## Readability
- **[consensus] Silhouette and pose first**, then base colors and form, details last (color-coded dummy).
- **[consensus] Define small characters through color blocks, not lines.** Head ≈ 1/3–1/2 of the figure height.
- **[consensus] Simplify for motion:** no large similarly colored areas next to each other; limbs equally thick and long in all directions ("one pixel counts").
- **[consensus] Check at 1x and 2x**, not only zoomed (`render_preview` shows both).
- **[style] Foot point:** an `origin` marker region at the bottom of the collision box; y-sorting by ground contact.

## Sources
- Cure, The Pixel Art Tutorial: https://pixeljoint.com/forum/forum_posts.asp?TID=11299
- Derek Yu: https://derekyu.com/makegames/pixelart.html
- Slynyrd Pixelblog 1 (palettes): https://www.slynyrd.com/blog/2018/1/10/pixelblog-1-color-palettes
- Slynyrd Pixelblog 22 (top-down characters): https://www.slynyrd.com/blog/2019/10/21/pixelblog-22-top-down-character-sprites
- Wayline: https://wayline.io/learn/pixel-art/1
- Saint11 (Pedro Medeiros): https://saint11.art/img/pixel-tutorials/Outlines.gif, https://saint11.art/img/pixel-tutorials/TopDownTricks.gif
