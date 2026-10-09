import { Ac as defineStd, Al as uniformOf, Ci as surfaceNoiseAt, Ec as vec2, Ic as animatedTime, Ml as p, Oo as colorLadder3, Ws as add, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, oc as local, qs as div, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var PI = 3.141592653589793;
const componentDefinition = defineStd({
	name: "Marble",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Classic marble swirl and vein texture using noise-warped sine waves",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#ffffff",
			transform: transformColor,
			description: "Base background color of the marble",
			ui: {
				type: "color",
				label: "Base",
				group: "Colors"
			}
		},
		colorB: {
			default: "#3a2d54",
			transform: transformColor,
			description: "Secondary marble tone",
			ui: {
				type: "color",
				label: "Tone",
				group: "Colors"
			}
		},
		colorC: {
			default: "#0f0f0f",
			transform: transformColor,
			description: "Deepest marble color",
			ui: {
				type: "color",
				label: "Depth",
				group: "Colors"
			}
		},
		scale: {
			default: 2,
			description: "Scale and density of the marble vein pattern",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 10,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		turbulence: {
			default: 10,
			description: "Amount of noise-driven distortion applied to the veins",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 50,
				step: .5,
				label: "Turbulence",
				group: "Effect"
			}
		},
		speed: {
			default: .05,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: .25,
				step: .005,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for pattern variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: (params) => {
		const { uv, viewport } = surfaceOf(params);
		const aspect = local(div(viewport.member("x"), max(viewport.member("y"), 1e-6)), "aspect");
		const t = local(animatedTime(params), "marbleTime");
		const seed = uniformOf(p("seed"), params);
		const pos = local(mul(vec2(mul(uv.member("x"), aspect), uv.member("y")), uniformOf(p("scale"), params)), "plane");
		const x = pos.member("x");
		const y = pos.member("y");
		const turbulence = local(add(add(surfaceNoiseAt(vec2(add(x, seed), add(y, t))), mul(surfaceNoiseAt(vec2(add(mul(x, 2), mul(seed, 1.7)), add(mul(y, 2), mul(t, .7)))), .5)), mul(surfaceNoiseAt(vec2(add(mul(x, 4), mul(seed, 3.1)), add(mul(y, 4), mul(t, 1.3)))), .25)), "marbleTurb");
		const vein = local(add(mul(sin(add(mul(x, PI), mul(turbulence, uniformOf(p("turbulence"), params)))), .5), .5), "veinT");
		const depth = add(mul(surfaceNoiseAt(vec2(add(mul(y, 1.5), mul(seed, 2)), mul(x, 1.2))), .3), .5);
		return colorLadder3({
			a: p("colorA"),
			b: p("colorB"),
			c: p("colorC"),
			space: p("colorSpace")
		})(smoothstep(.3, .6, vein), mul(smoothstep(.6, .9, vein), depth), params);
	}
});
var Marble_default = componentDefinition;
export { componentDefinition as n, Marble_default as t };
