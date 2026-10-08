import { Ac as defineStd, Cn as rgbSplit, Fn as vignetteOverlay, Ml as p, Tn as scanlines, Ut as adjust, _n as phosphorMask, pn as opaque, sn as gatherStack } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "CRTScreen",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Retro CRT monitor simulation with scanlines",
	props: {
		pixelSize: {
			default: 128,
			description: "Density of the RGB phosphor stripes — about half this many across the canvas width (higher = smaller, finer pixels)",
			ui: {
				type: ["range", "map"],
				min: 8,
				max: 128,
				step: 1,
				label: "Pixel Size",
				group: "Effect"
			}
		},
		colorShift: {
			default: 1,
			description: "Chromatic aberration amount",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Color Shift",
				group: "Effect"
			}
		},
		scanlineIntensity: {
			default: .3,
			description: "Strength of horizontal scanlines",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Scanline Intensity",
				group: "Effect"
			}
		},
		scanlineFrequency: {
			default: 200,
			description: "Number of scanlines across screen",
			ui: {
				type: ["range", "map"],
				min: 100,
				max: 800,
				step: 10,
				label: "Scanline Frequency",
				group: "Effect"
			}
		},
		brightness: {
			default: 1,
			description: "Screen brightness boost",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 2,
				step: .1,
				label: "Brightness",
				group: "Adjustments"
			}
		},
		contrast: {
			default: 1,
			description: "Screen contrast enhancement",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 2,
				step: .1,
				label: "Contrast",
				group: "Adjustments"
			}
		},
		vignetteIntensity: {
			default: 1,
			description: "Strength of corner darkening effect (0 = off)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Vignette Intensity",
				group: "Effect"
			}
		},
		vignetteRadius: {
			default: .5,
			description: "How far the vignette extends inward (0 = edges only, 1 = reaches center)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Vignette Radius",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(rgbSplit({ amount: p("colorShift") }), [
		adjust({
			brightness: p("brightness"),
			contrast: p("contrast")
		}),
		scanlines({
			frequency: p("scanlineFrequency"),
			intensity: p("scanlineIntensity")
		}),
		phosphorMask({ pitch: p("pixelSize") }),
		vignetteOverlay({
			radius: p("vignetteRadius"),
			intensity: p("vignetteIntensity")
		}),
		opaque()
	])
});
var CRTScreen_default = componentDefinition;
export { componentDefinition as n, CRTScreen_default as t };
