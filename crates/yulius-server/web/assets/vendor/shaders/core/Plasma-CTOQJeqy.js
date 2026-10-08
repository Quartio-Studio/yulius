import { Ac as defineStd, Fo as noiseField, Ml as p, Qo as warped, Vo as scaledVolume, Xo as tone, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Plasma",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Animated effect of glowing plasma",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		density: {
			default: 2,
			description: "Density of the plasma pattern",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 4,
				step: .1,
				label: "Density",
				group: "Effect"
			}
		},
		speed: {
			default: 2,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Effect"
			}
		},
		intensity: {
			default: 1.5,
			description: "Brightness and spread of the plasma glow",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 3,
				step: .1,
				label: "Intensity",
				group: "Colors"
			}
		},
		warp: {
			default: .4,
			description: "How much the flow distorts and swirls",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Warp",
				group: "Effect"
			}
		},
		contrast: {
			default: 1,
			description: "Push darks darker and lights lighter",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .1,
				label: "Contrast",
				group: "Colors"
			}
		},
		balance: {
			default: 50,
			description: "Skew color balance toward A (higher) or B (lower)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Balance",
				group: "Colors"
			}
		},
		colorA: {
			default: "#7018be",
			description: "Primary color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#000000",
			description: "Secondary color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
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
	paint: rampOver(tone(scaledVolume(warped(noiseField("mx3"), {
		amount: p("warp"),
		amountScale: 4,
		timeScale: .125
	}), { scale: p("density") }), {
		glow: p("intensity"),
		contrast: p("contrast"),
		balance: p("balance"),
		invert: true
	}), stops(p("colorSpace")))
});
var Plasma_default = componentDefinition;
export { componentDefinition as n, Plasma_default as t };
