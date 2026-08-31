// Paint-swirl background. The effect is localthunk's Balatro shader
// (https://www.playbalatro.com), rewritten to composite behind terminal text
// and to take its colours from the background each pixel sits on: the
// terminal's own default background, or one of wt's per-window tints.

#define SPIN_ROTATION -2.0
#define SPIN_SPEED 0.5
#define OFFSET vec2(0.0)
#define CONTRAST 3.5
#define LIGTHING 0.12
#define SPIN_AMOUNT 0.25
#define PIXEL_FILTER 745.0
#define SPIN_EASE 1.0
#define IS_ROTATE false

// How far the swirl may pull a pixel away from the plain background colour.
// At 0 the effect disappears. This is the dial to reach for first.
const float SWIRL_STRENGTH = 0.25;

// These set the swirl's own colour before SWIRL_STRENGTH scales it back, so
// raising them widens the hue range rather than making the effect stronger.
const float SWIRL_SATURATION = 0.5;
const float SWIRL_BRIGHTNESS = 0.35;

// Distance around the hue wheel between the swirl's two main colours. The
// original's red and blue are half a turn apart, which is far too loud here.
const float HUE_SPREAD = 0.08;

// Below this the background is near-grey and carries no hue to build on.
const float MIN_USABLE_SATURATION = 0.15;

// The original's own two hues, used when the background has none to offer.
const float FALLBACK_HUE_1 = 0.009;
const float FALLBACK_HUE_2 = 0.568;

// How close a pixel must sit to a background colour to count as background.
// Glyphs are antialiased toward that colour, so the outer bound feathers their
// edges rather than leaving a hard fringe around every character.
const float KEY_EXACT = 0.02;
const float KEY_FEATHER = 0.10;

// wt's worktree tints (wt-cli/src/color.rs PALETTE). Inside tmux wt paints a
// window's cells with one of these via tmux's `window-style` option, which
// the default-background uniform never reflects — so each tint is keyed per
// pixel, and the swirl takes its hue from whichever background that pixel
// sits on. A test in color.rs keeps this list in sync with the palette.
const vec3 WT_TINTS[12] = vec3[12](
    vec3(0x17, 0x0a, 0x0a) / 255.0, // #170a0a red
    vec3(0x17, 0x10, 0x0a) / 255.0, // #17100a orange
    vec3(0x17, 0x15, 0x0a) / 255.0, // #17150a yellow
    vec3(0x11, 0x17, 0x0a) / 255.0, // #11170a lime
    vec3(0x0a, 0x17, 0x0c) / 255.0, // #0a170c green
    vec3(0x0a, 0x17, 0x14) / 255.0, // #0a1714 teal
    vec3(0x0a, 0x15, 0x17) / 255.0, // #0a1517 cyan
    vec3(0x0a, 0x10, 0x17) / 255.0, // #0a1017 blue
    vec3(0x0c, 0x0a, 0x17) / 255.0, // #0c0a17 indigo
    vec3(0x12, 0x0a, 0x17) / 255.0, // #120a17 purple
    vec3(0x17, 0x0a, 0x15) / 255.0, // #170a15 magenta
    vec3(0x17, 0x0a, 0x0f) / 255.0  // #170a0f pink
);

vec3 rgbToHsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    const float eps = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + eps)), d / (q.x + eps), q.x);
}

vec3 hsvToRgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

vec3 effect(vec2 screenSize, vec2 screen_coords, vec4 colour1, vec4 colour2, vec4 colour3) {
    float pixel_size = length(screenSize.xy) / PIXEL_FILTER;
    vec2 uv = (floor(screen_coords.xy * (1. / pixel_size)) * pixel_size - 0.5 * screenSize.xy) / length(screenSize.xy) - OFFSET;
    float uv_len = length(uv);

    float speed = (SPIN_ROTATION * SPIN_EASE * 0.2);
    if (IS_ROTATE) {
        speed = iTime * speed;
    }
    speed += 302.2;
    float new_pixel_angle = atan(uv.y, uv.x) + speed - SPIN_EASE * 20. * (1. * SPIN_AMOUNT * uv_len + (1. - 1. * SPIN_AMOUNT));
    vec2 mid = (screenSize.xy / length(screenSize.xy)) / 2.;
    uv = (vec2((uv_len * cos(new_pixel_angle) + mid.x), (uv_len * sin(new_pixel_angle) + mid.y)) - mid);

    uv *= 30.;
    speed = iTime * (SPIN_SPEED);
    vec2 uv2 = vec2(uv.x + uv.y);

    for (int i = 0; i < 5; i++) {
        uv2 += sin(max(uv.x, uv.y)) + uv;
        uv += 0.5 * vec2(cos(5.1123314 + 0.353 * uv2.y + speed * 0.131121), sin(uv2.x - 0.113 * speed));
        uv -= 1.0 * cos(uv.x + uv.y) - 1.0 * sin(uv.x * 0.711 - uv.y);
    }

    float contrast_mod = (0.25 * CONTRAST + 0.5 * SPIN_AMOUNT + 1.2);
    float paint_res = min(2., max(0., length(uv) * (0.035) * contrast_mod));
    float c1p = max(0., 1. - contrast_mod * abs(1. - paint_res));
    float c2p = max(0., 1. - contrast_mod * abs(paint_res));
    float c3p = 1. - min(1., c1p + c2p);
    float light = (LIGTHING - 0.2) * max(c1p * 5. - 4., 0.) + LIGTHING * max(c2p * 5. - 4., 0.);
    vec4 paint = (0.3 / CONTRAST) * colour1
        + (1. - 0.3 / CONTRAST) * (colour1 * c1p + colour2 * c2p + vec4(c3p * colour3.rgb, c3p * colour1.a))
        + light;
    return paint.rgb;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 terminal = texture(iChannel0, fragCoord / iResolution.xy);

    // The background this pixel sits on: whichever of the terminal default
    // and the wt tints it is closest to. Two visible windows with different
    // tints each swirl in their own hue.
    vec3 key = iBackgroundColor;
    float keyDistance = distance(terminal.rgb, key);
    for (int i = 0; i < 12; i++) {
        float tintDistance = distance(terminal.rgb, WT_TINTS[i]);
        if (tintDistance < keyDistance) {
            keyDistance = tintDistance;
            key = WT_TINTS[i];
        }
    }

    vec3 keyHsv = rgbToHsv(key);
    bool tinted = keyHsv.y >= MIN_USABLE_SATURATION;
    float hue1 = tinted ? keyHsv.x : FALLBACK_HUE_1;
    float hue2 = tinted ? fract(keyHsv.x + HUE_SPREAD) : FALLBACK_HUE_2;

    vec4 colour1 = vec4(hsvToRgb(vec3(hue1, SWIRL_SATURATION, SWIRL_BRIGHTNESS)), 1.0);
    vec4 colour2 = vec4(hsvToRgb(vec3(hue2, SWIRL_SATURATION, SWIRL_BRIGHTNESS * 0.85)), 1.0);
    vec4 colour3 = vec4(hsvToRgb(vec3(hue1, SWIRL_SATURATION * 0.5, SWIRL_BRIGHTNESS * 0.25)), 1.0);

    vec3 swirl = effect(iResolution.xy, fragCoord, colour1, colour2, colour3);

    // Sit the pattern close to the plain background so it reads as faint
    // texture rather than as a picture behind the text.
    swirl = mix(key, swirl, SWIRL_STRENGTH);

    float isBackground = 1.0 - smoothstep(KEY_EXACT, KEY_FEATHER, keyDistance);

    fragColor = vec4(mix(terminal.rgb, swirl, isBackground), terminal.a);
}
