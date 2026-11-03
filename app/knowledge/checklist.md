# Checklist before handing work over

Go through this before handing over any graphic or animation. Details in `fundamentals`, `animation-principles` and `pixel-animation`; working rules in `workflow`.

## Still image
1. Silhouette readable as a shadow shape? Checked at 1x and 2x (`render_preview`)?
2. Lines with even steps, no doubles, no orphan pixels?
3. One light direction, no pillow shading, no banding?
4. Only colors of the given document palette, no blend-color fringes?
5. Outer outline closed (in the project's outline color), inner lines in the dark material tone?
6. Limbs and equipment as thick and long as in the other directions?

## Animation
1. Key poses (contact F1/F5, passing F3/F7 in an 8-frame walk) come first and are readable.
2. Per step 1 low and 1 high, 1–2 px, triangle wave; less bob when walking away.
3. Arms opposite, hands and feet on arcs, supporting foot without sliding on the ground line (`render_animation` with `track`).
4. Rigid items pixel-identical, only moved as a whole; loose parts lag 1 frame.
5. No pixel changes in static areas (difference row of `render_animation`).
6. Mirror only without asymmetry; equipment without jumps across direction changes.
7. Loop watched at 1x and in real time (editor preview or GIF), not only the filmstrip. Frame durations uneven where extremes should be held.
8. Claim only what you actually looked at.

## Collaboration
- Changed only what the user named; marks handled and removed (`clear_marks`).
- Approved state protected (`snapshot`, drafts under their own name).
- Lessons the user confirmed recorded with `add_learning`.
