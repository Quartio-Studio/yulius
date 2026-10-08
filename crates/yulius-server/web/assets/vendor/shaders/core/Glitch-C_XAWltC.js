import { Ac as defineStd, Ml as p, Qt as colorBarFills, ln as glitchFrame, rn as distortedScanlines, sn as gatherStack, un as glitchRgbSplit } from "./std-ClftfWpF.js";
var glitchGeometry = glitchFrame({
	intensity: p("intensity"),
	speed: p("speed"),
	blockDensity: p("blockDensity"),
	mirrorAmount: p("mirrorAmount")
});
const componentDefinition = defineStd({
	name: "Glitch",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Digital glitch that melts pixels and distorts colors",
	props: {
		intensity: {
			default: .5,
			description: "Overall glitch strength and frequency of glitch bursts",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "How fast the glitch pattern evolves",
			ui: {
				type: "range",
				min: .1,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Effect"
			}
		},
		rgbShift: {
			default: 5,
			description: "Amount of chromatic aberration (RGB channel splitting)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .5,
				label: "RGB Shift",
				group: "Effect"
			}
		},
		blockDensity: {
			default: 10,
			description: "Base number of horizontal glitch bands",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 50,
				step: 1,
				label: "Block Density",
				group: "Effect"
			}
		},
		colorBarIntensity: {
			default: .2,
			description: "Intensity of vivid neon color bar overlay in glitch regions",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Color Bars",
				group: "Effect"
			}
		},
		mirrorAmount: {
			default: .3,
			description: "Chance of glitch blocks showing mirrored/flipped content",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Mirror",
				group: "Effect"
			}
		},
		scanlineIntensity: {
			default: .2,
			description: "Visibility of CRT-style horizontal scanlines in distorted areas",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Scanlines",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(glitchRgbSplit(glitchGeometry, { shift: p("rgbShift") }), [colorBarFills(glitchGeometry, { intensity: p("colorBarIntensity") }), distortedScanlines(glitchGeometry, { intensity: p("scanlineIntensity") })])
});
var Glitch_default = componentDefinition;
export { componentDefinition as n, Glitch_default as t };
