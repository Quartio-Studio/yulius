import { $o as warpedPoint, Ac as defineStd, Al as uniformOf, Dc as vec3, Ec as vec2, Gs as clamp, Ho as scatterField, Ic as animatedTime, Ko as standardPalette, Ks as cos, Ml as p, Tc as sub, Tr as dithered, Uo as scatteredAnchors, Wf as vec4f, Ws as add, Yf as arrayOf, Ys as exp, _c as rotate2, ac as length, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, es as wrapRamp, fd as isMobileGpuViewport, ji as volumeNoiseAt, oc as local, ou as colorStopsPropConfig, qs as div, yc as sin, ys as crosses } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var DOMAIN_SCALE = 1.6;
var WARP_AMOUNT = .48;
var WARP_RATE = .125;
var SWIRL_MAX = 3;
var VAR_FREQ = 1.9;
var VAR_STRENGTH = 1.4;
var VAR_RATE = .045;
var WRAP_CYCLES = 9;
var DEFAULT_STOPS = [
	{
		color: "#1a0533",
		position: 0
	},
	{
		color: "#6d2fd1",
		position: .26
	},
	{
		color: "#e04b9e",
		position: .52
	},
	{
		color: "#ff8c42",
		position: .76
	},
	{
		color: "#ffdf8e",
		position: 1
	}
];
const componentDefinition = defineStd({
	name: "MeshGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Flowing mesh gradient of soft drifting color swaths whose seams wrap through the palette",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	extraFields: { meshAnchors: {
		schema: arrayOf(vec4f, 4),
		initial: new Array(16).fill(0)
	} },
	props: {
		colorA: {
			default: "#1a0533",
			transform: transformColor,
			description: "First gradient color (two-color fallback when stops are cleared)",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffdf8e",
			transform: transformColor,
			description: "Second gradient color (two-color fallback when stops are cleared)",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: {
			...colorStopsPropConfig(),
			default: DEFAULT_STOPS
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		count: {
			default: 5,
			description: "Number of color points scattered across the canvas",
			ui: {
				type: "range",
				min: 2,
				max: 8,
				step: 1,
				label: "Points",
				group: "Effect"
			}
		},
		smoothness: {
			default: 2,
			description: "How smoothly the color points blend into each other",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .01,
				label: "Smoothness",
				group: "Effect"
			}
		},
		variation: {
			default: .35,
			recompile: crosses(0),
			description: "Varies edge softness across the canvas — some color boundaries crisp, others diffuse",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Variation",
				group: "Effect"
			}
		},
		swirl: {
			default: .3,
			description: "Vortex rotation that spirals the field around the frame centre — negative values spin the other way",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Swirl",
				group: "Effect"
			}
		},
		drift: {
			default: .5,
			description: "How far the color points wander from their home positions",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Drift",
				group: "Effect"
			}
		},
		wrapping: {
			default: 0,
			description: "Wraps the palette back through itself where colors meet — banded seams where the field pinches, smooth elsewhere",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Wrapping",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for the point layout and palette assignment",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		}
	},
	paint: (params) => {
		const { uv, viewport } = surfaceOf(params);
		const aspect = local(div(viewport.member("x"), max(viewport.member("y"), 1e-6)), "aspect");
		const t = local(animatedTime(params), "meshTime");
		const seed = uniformOf(p("seed"), params);
		const centred = local(vec2(sub(mul(uv.member("x"), aspect), mul(aspect, .5)), sub(uv.member("y"), .5)), "meshCentred");
		const swirlAngle = local(mul(mul(-SWIRL_MAX, uniformOf(p("swirl"), params)), smoothstep(0, 1, length(centred))), "swirlAngle");
		const swirled = local(rotate2(centred, cos(swirlAngle), sin(swirlAngle)), "meshRot");
		const uvS = local(vec2(add(swirled.member("x"), mul(aspect, .5)), add(swirled.member("y"), .5)), "meshSwirled");
		const warped = local(warpedPoint({
			at: vec3(mul(uvS.member("x"), DOMAIN_SCALE), mul(uvS.member("y"), DOMAIN_SCALE), mul(seed, 7.31)),
			time: mul(t, WARP_RATE),
			amount: WARP_AMOUNT,
			levels: isMobileGpuViewport() ? 1 : 2
		}), "meshWarp");
		const q = local(vec2(div(warped.member("x"), DOMAIN_SCALE), div(warped.member("y"), DOMAIN_SCALE)), "meshQ");
		const basePower = div(8, add(uniformOf(p("smoothness"), params), .5));
		const variation = params.propValues.variation;
		const softness = variation !== void 0 && variation !== 0 ? (() => {
			return clamp(mul(basePower, exp(mul(mul(volumeNoiseAt(vec3(mul(q.member("x"), VAR_FREQ), mul(q.member("y"), VAR_FREQ), add(mul(seed, 3.17), mul(t, VAR_RATE)))), uniformOf(p("variation"), params)), VAR_STRENGTH))), .6, 16);
		})() : clamp(basePower, .6, 16);
		const anchors = scatteredAnchors(params, {
			count: p("count"),
			seed: p("seed"),
			drift: p("drift"),
			field: "meshAnchors"
		});
		return dithered(standardPalette(wrapRamp({
			t: scatterField({
				at: q,
				count: uniformOf(p("count"), params),
				seed,
				drift: uniformOf(p("drift"), params),
				aspect,
				time: t,
				softness,
				anchors
			}),
			cycles: add(1, mul(uniformOf(p("wrapping"), params), WRAP_CYCLES))
		}), params), params);
	}
});
var MeshGradient_default = componentDefinition;
export { componentDefinition as n, MeshGradient_default as t };
