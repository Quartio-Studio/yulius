import { Ac as defineStd, Cc as sqrt, Dc as vec3, Ec as vec2, Ic as animatedTime, In as flashes, Js as dot, Jt as brightestNear, Oc as vec4, Sc as splat3, Ss as isZero, Va as starGlint, Ws as add, bc as smoothstep, cc as max, dc as mul, fo as scatterPoints, lc as min, oc as local, qs as div, sn as gatherStack, uc as mix, wc as step, ys as crosses } from "./std-ClftfWpF.js";
var GRID_SCALES = [1, 2 / 3];
var THRESHOLD_BAND = .12;
var GLINT = {
	core: .9,
	rayWidth: 1.6,
	rayLength: .45,
	rays: .25
};
var BLINK = {
	minRate: .8,
	rateSpread: 1.2,
	sharpness: 8
};
var BRIGHT_SHARE = .45;
var DIM_LEVEL = .35;
var HUE_GAIN = .6;
var LUMA = [
	.2126,
	.7152,
	.0722
];
var EXPAND_TAPS = 4;
function glintLayer(params, px, gridScale, layer, t) {
	const u = params.uniforms;
	const cellSize = local(mul(u.size, gridScale), `sparkleCell${layer}`);
	const expanding = (params.propValues.expand ?? 0) > 0;
	const expandUV = local(div(vec2(u.expand, u.expand), params.ctx.logicalViewportSize), `sparkleExpand${layer}`);
	const rayRate = local(div(GLINT.rayLength, max(u.rayLength, .01)), `sparkleRays${layer}`);
	return scatterPoints(px, {
		cellSize,
		jitter: .6,
		seed: add(u.seed, layer * .31)
	}).map(({ point, offset, random }) => {
		const source = brightestNear(params.sampleStraight, div(point, params.ctx.logicalViewportSize), {
			radius: expandUV,
			taps: expanding ? EXPAND_TAPS : 0,
			angle: layer * 45
		});
		const presence = smoothstep(u.threshold, add(u.threshold, THRESHOLD_BAND), sqrt(source.luma));
		const blink = flashes(t, {
			rate: add(BLINK.minRate, mul(random.member("z"), BLINK.rateSpread)),
			offset: mul(random.member("x"), 6.283185307179586),
			sharpness: BLINK.sharpness
		});
		const level = add(DIM_LEVEL, mul(1 - DIM_LEVEL, step(BRIGHT_SHARE, random.member("z"))));
		const star = starGlint(offset, {
			core: GLINT.core,
			rayWidth: GLINT.rayWidth,
			rayLength: rayRate,
			rays: GLINT.rays,
			reach: mul(cellSize, .5)
		});
		const rgb = local(source.color.member("rgb"), "sparkleRgb");
		const hue = mul(div(rgb, max(dot(rgb, vec3(...LUMA)), .001)), HUE_GAIN);
		return mul(mix(splat3(1), hue, u.colorize), mul(mul(mul(star, blink), presence), mul(level, u.intensity)));
	}).reduce((a, b) => add(a, b));
}
const componentDefinition = defineStd({
	name: "Sparkle",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Twinkling star glints over the bright parts of the layer inside",
	animatedTime: { speed: "speed" },
	props: {
		size: {
			default: 100,
			description: "Spacing between glints, in CSS pixels (smaller means more glints)",
			ui: {
				type: ["range", "map"],
				min: 10,
				max: 200,
				step: 1,
				label: "Spacing",
				group: "Effect"
			}
		},
		intensity: {
			default: 10,
			description: "Brightness of the glints",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		threshold: {
			default: .2,
			description: "How bright the layer must be before glints appear: 0 allows all but black, 0.5 is mid-grey and up, 1 only pure white",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Threshold",
				group: "Effect"
			}
		},
		expand: {
			default: 0,
			description: "Lets glints appear up to this many CSS pixels outside the bright areas",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Expand",
				group: "Effect"
			}
		},
		rayLength: {
			default: 5,
			description: "Length of the four rays around each glint (they fade out by half the spacing)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "Ray Length",
				group: "Effect"
			}
		},
		colorize: {
			default: .45,
			description: "How much each glint takes on the colour beneath it (0 is pure white)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Colorize",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "How fast the glints twinkle",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .05,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Re-rolls where the glints sit and when they blink",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Animation"
			}
		}
	},
	identityWhen: isZero("intensity"),
	effect: gatherStack((params) => {
		const px = local(mul(params.ctx.uv, params.ctx.logicalViewportSize), "sparklePx");
		const t = local(animatedTime(params), "sparkleT");
		const glow = local(GRID_SCALES.map((scale, layer) => glintLayer(params, px, scale, layer, t)).reduce((a, b) => add(a, b)), "sparkleGlow");
		const child = local(params.texture.sample(params.ctx.uv), "sparkleChild");
		const spill = max(max(glow.member("r"), glow.member("g")), glow.member("b"));
		return vec4(add(child.member("rgb"), glow), min(add(child.member("a"), spill), 1));
	}, []),
	missingChildMessage: "You must pass a child component into the Sparkle shader."
});
var Sparkle_default = componentDefinition;
export { componentDefinition as n, Sparkle_default as t };
