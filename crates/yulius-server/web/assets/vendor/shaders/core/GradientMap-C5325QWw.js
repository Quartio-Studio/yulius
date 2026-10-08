import { Ac as defineStd, Ml as p, Ss as isZero, jt as gradientMap, ys as crosses } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "GradientMap",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Maps source luminance through an animated color gradient (Photoshop-style gradient map)",
	props: {
		palette: {
			default: "rainbow",
			compileTime: true,
			description: "Built-in cosine palette, or Custom Colors to pick your own stops",
			ui: {
				type: "select",
				options: [
					{
						label: "Rainbow",
						value: "rainbow"
					},
					{
						label: "Sunset",
						value: "sunset"
					},
					{
						label: "Ocean",
						value: "ocean"
					},
					{
						label: "Fire",
						value: "fire"
					},
					{
						label: "Pastel",
						value: "pastel"
					},
					{
						label: "Neon",
						value: "neon"
					},
					{
						label: "Custom Colors",
						value: "custom"
					}
				],
				label: "Palette",
				group: "Colors"
			}
		},
		colorLow: {
			default: "#1a0b2e",
			transform: transformColor,
			description: "Color for the darkest tones",
			ui: {
				type: "color",
				label: "Shadows",
				group: "Colors",
				condition: { palette: "custom" }
			}
		},
		colorMid: {
			default: "#e94560",
			transform: transformColor,
			description: "Color for the midtones",
			ui: {
				type: "color",
				label: "Midtones",
				group: "Colors",
				condition: { palette: "custom" }
			}
		},
		colorHigh: {
			default: "#f9ed69",
			transform: transformColor,
			description: "Color for the brightest tones",
			ui: {
				type: "color",
				label: "Highlights",
				group: "Colors",
				condition: { palette: "custom" }
			}
		},
		speed: {
			default: .15,
			description: "Gradient animation speed (0 = static). The gradient cycles seamlessly.",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 2,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		contrast: {
			default: 1,
			description: "Steepness of the luminance-to-gradient mapping",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .01,
				label: "Contrast",
				group: "Effect"
			}
		},
		blackPoint: {
			default: 0,
			description: "Input shadow clip — luminance at/below this maps to the start of the gradient",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Black Point",
				group: "Effect"
			}
		},
		whitePoint: {
			default: 1,
			description: "Input highlight clip — luminance at/above this maps to the end of the gradient",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "White Point",
				group: "Effect"
			}
		},
		strength: {
			default: 1,
			description: "Blend between the original image (0) and the gradient-mapped result (1)",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "oklch",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for interpolating custom colors",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors",
				condition: { palette: "custom" }
			}
		}
	},
	effect: gradientMap({
		palette: p("palette"),
		colorLow: p("colorLow"),
		colorMid: p("colorMid"),
		colorHigh: p("colorHigh"),
		speed: p("speed"),
		contrast: p("contrast"),
		blackPoint: p("blackPoint"),
		whitePoint: p("whitePoint"),
		strength: p("strength"),
		colorSpace: p("colorSpace")
	}),
	identityWhen: isZero("strength"),
	missingChildMessage: "You must pass a child component into the GradientMap shader."
});
var GradientMap_default = componentDefinition;
export { componentDefinition as n, GradientMap_default as t };
