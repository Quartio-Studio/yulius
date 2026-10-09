import { Ac as defineStd, Ml as p, Ot as duotone } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Duotone",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Map colors to two tones based on luminance",
	props: {
		colorA: {
			default: "#ff0000",
			transform: transformColor,
			description: "First color (used for darker areas)",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#023af4",
			transform: transformColor,
			description: "Second color (used for brighter areas)",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		blend: {
			default: .5,
			description: "Blend point between the two colors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Blend",
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
	effect: duotone({
		colorA: p("colorA"),
		colorB: p("colorB"),
		blend: p("blend"),
		colorSpace: p("colorSpace")
	})
});
var Duotone_default = componentDefinition;
export { componentDefinition as n, Duotone_default as t };
