import { Ac as defineStd, Ml as p, Vt as tritone } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Tritone",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Map colors to three tones: shadows, midtones, highlights",
	props: {
		colorA: {
			default: "#ce1bea",
			transform: transformColor,
			description: "First color (used for shadows/darkest areas)",
			ui: {
				type: "color",
				label: "Color A (Shadows)",
				group: "Colors"
			}
		},
		colorB: {
			default: "#2fff00",
			transform: transformColor,
			description: "Second color (used for midtones)",
			ui: {
				type: "color",
				label: "Color B (Midtones)",
				group: "Colors"
			}
		},
		colorC: {
			default: "#ffff00",
			transform: transformColor,
			description: "Third color (used for highlights/brightest areas)",
			ui: {
				type: "color",
				label: "Color C (Highlights)",
				group: "Colors"
			}
		},
		blendMid: {
			default: .5,
			description: "Midpoint position between the three colors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Midpoint",
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
	effect: tritone({
		colorA: p("colorA"),
		colorB: p("colorB"),
		colorC: p("colorC"),
		blendMid: p("blendMid"),
		colorSpace: p("colorSpace")
	})
});
var Tritone_default = componentDefinition;
export { componentDefinition as n, Tritone_default as t };
