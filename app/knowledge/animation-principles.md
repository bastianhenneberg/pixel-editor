# The 12 animation principles in pixels

After Thomas & Johnston, each translated to low resolution. Markers as in `fundamentals`.

1. **Squash & stretch [consensus]:** mass stays constant. 1 px wider means 1 px lower. Rigid parts (armour, sword) do not deform. In pixels: 1–2 px torso height on contact/take-off.
2. **Anticipation [consensus]:** preparation against the direction of the action. Attack: long wind-up, hit 1–2 frames, then recovery.
3. **Staging:** the pose reads as a silhouette; leave out what is not needed.
4. **Pose to pose [consensus]** for game cycles: key poses → breakdowns → in-betweens. With 8 frames almost every frame is a key or breakdown.
5. **Follow-through / overlap [consensus]:** loose parts (hair, cape, straps, tassels) arrive 1–2 frames later and swing out. Delay head/face against the body ("head delay").
6. **Slow in / slow out [consensus]:** hold extremes longer. In pixels via **frame duration** (`set_frame_duration` per frame) or uneven steps (e.g. 1 down, 1 down, 2 up).
7. **Arcs [consensus]:** hands, feet and head travel on arcs around joints; plan positions across frames as an arc, not linearly (`render_animation` with `track` plots the path).
8. **Secondary action:** supports, does not compete (arm swing, blinking).
9. **Timing [consensus]:** timing = how long, spacing = how far per frame. Spacing is quantised in pixels (0/1/2 px), so frame duration carries more.
10. **Exaggeration [consensus for small sprites]:** exaggerate, or 1 px gets lost (pull the fist up almost like a punch when walking).
11. **Solid drawing:** volume constant across all frames; no mirror-identical "twin poses".
12. **Appeal:** simple, clear shapes, big heads.

## Avoid uniformity [consensus]
- Even timing looks robotic: hold the extremes.
- Head height as a **triangle wave** (quick up, short at the top), not a sine.

## Sources
- https://en.wikipedia.org/wiki/Twelve_basic_principles_of_animation
- https://saint11.art/img/pixel-tutorials/Squash.gif
- https://pixnote.net/en/learn/animation
- Slynyrd Pixelblog 50 (walk): https://www.slynyrd.com/blog/2024/5/24/pixelblog-50-human-walk-cycle
- Slynyrd Pixelblog 55 (top-down animation): https://www.slynyrd.com/blog/2025/3/24/pixelblog-55-top-down-character-animation
- Reading notes on Williams, "The Animator's Survival Kit": https://s184753.blogspot.com/2019/01/more-research-animators-survival-kit.html
