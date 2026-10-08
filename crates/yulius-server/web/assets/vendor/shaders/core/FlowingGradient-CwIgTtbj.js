import { Ac as defineStd, Al as uniformOf, Dc as vec3, Ec as vec2, Gs as clamp, Ic as animatedTime, Ks as cos, Ml as p, Oc as vec4, Ro as quadBlend, Sc as splat3, Tc as sub, Uf as vec3f, Us as abs, Ws as add, Zo as warpStep, _c as rotate2, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, ji as volumeNoiseAt, mc as pow, oc as local, qs as div, uc as mix, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var FOUR_PI = 4 * Math.PI;
var WARP_LEVELS = [{
	scale: 2,
	offsets: [[5.2, 1.3], [1.7, 9.2]],
	reach: .25
}, {
	scale: 1.5,
	offsets: [[8.3, 2.8], [3.1, 7.4]],
	reach: .2
}];
const componentDefinition = defineStd({
	name: "FlowingGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Liquid silk gradient with organic flowing color bands",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	extraFields: {
		convA: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		},
		convB: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		},
		convC: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		},
		convD: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		}
	},
	props: {
		colorA: {
			default: "#0a0015",
			transform: transformColor,
			description: "Deep background color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#6b17e6",
			transform: transformColor,
			description: "Primary accent color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		colorC: {
			default: "#ff4d6a",
			transform: transformColor,
			description: "Secondary accent color",
			ui: {
				type: "color",
				label: "Color C",
				group: "Colors"
			}
		},
		colorD: {
			default: "#ff6b35",
			transform: transformColor,
			description: "Tertiary accent color",
			ui: {
				type: "color",
				label: "Color D",
				group: "Colors"
			}
		},
		colorSpace: {
			default: "oklch",
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
		speed: {
			default: 1,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .1,
				label: "Speed",
				group: "Effect"
			}
		},
		distortion: {
			default: .5,
			description: "Organic distortion intensity",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Distortion",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for variation",
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
		const t = local(mul(animatedTime(params), .075), "flowTime");
		const seedOff = local(mul(uniformOf(p("seed"), params), .17), "flowSeed");
		const strength = local(uniformOf(p("distortion"), params), "flowStrength");
		const centred = local(sub(uv, .5), "flowCentred");
		const angle = local(mul(mul(volumeNoiseAt(vec3(add(mul(centred.member("x"), .8), mul(t, .4)), sub(mul(centred.member("y"), .8), mul(t, .25)), seedOff)), FOUR_PI), strength), "flowAngle");
		const spun = local(rotate2(vec2(centred.member("x"), div(centred.member("y"), aspect)), cos(angle), sin(angle)), "flowSpun");
		const rot = local(vec2(spun.member("x"), mul(spun.member("y"), aspect)), "flowRot");
		const shearT = mul(t, 2);
		const shearX = local(add(rot.member("x"), mul(div(sin(add(mul(rot.member("y"), 5), shearT)), 50), strength)), "flowShearX");
		const level1 = local(warpStep({
			at: vec2(shearX, add(rot.member("y"), mul(div(sin(add(mul(shearX, 7.5), shearT)), 25), strength))),
			scale: WARP_LEVELS[0].scale,
			z: add(mul(t, .5), seedOff),
			offsets: WARP_LEVELS[0].offsets,
			reach: WARP_LEVELS[0].reach,
			strength
		}), "flowWarp1");
		const tz = local(add(mul(t, .3), seedOff), "flowTz");
		const level2 = local(warpStep({
			at: level1.member("xy"),
			scale: WARP_LEVELS[1].scale,
			z: tz,
			offsets: WARP_LEVELS[1].offsets,
			reach: WARP_LEVELS[1].reach,
			strength
		}), "flowWarp2");
		const zoneNoise = volumeNoiseAt(vec3(mul(level2.member("x"), 1.8), mul(level2.member("y"), 1.8), tz));
		const edgeHardness = local(pow(add(mul(level1.member("z"), .5), .5), 1.5), "flowHardness");
		const edgeWidth = local(mix(.5, .008, edgeHardness), "flowEdge");
		const t1 = local(smoothstep(mul(edgeWidth, -1), edgeWidth, zoneNoise), "flowT1");
		const t2 = local(smoothstep(mul(edgeWidth, -1), edgeWidth, level1.member("w")), "flowT2");
		const base = local(quadBlend({
			a: p("colorA"),
			b: p("colorB"),
			c: p("colorC"),
			d: p("colorD"),
			space: p("colorSpace")
		})(t1, t2, params), "flowBase");
		const foldMask = (blend) => sub(1, abs(sub(mul(blend, 2), 1)));
		const foldHighlight = mul(max(foldMask(t1), foldMask(t2)), edgeHardness);
		const litScale = mul(add(mul(add(mul(level2.member("z"), .5), .5), .12), .94), add(1, mul(foldHighlight, .35)));
		return vec4(clamp(mul(base.member("rgb"), litScale), splat3(0), splat3(1)), base.member("a"));
	}
});
var FlowingGradient_default = componentDefinition;
export { componentDefinition as n, FlowingGradient_default as t };
