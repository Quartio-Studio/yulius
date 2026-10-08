import { Ac as defineStd, Bt as tint, Ml as p } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Tint",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Apply a color tint to the image",
	props: {
		color: {
			default: "#ff8800",
			transform: transformColor,
			description: "Tint color",
			ui: {
				type: "color",
				label: "Tint Color",
				group: "Colors"
			}
		},
		amount: {
			default: .5,
			description: "Tint amount (0 = no tint, 1 = full tint)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Amount",
				group: "Effect"
			}
		},
		preserveLuminosity: {
			default: true,
			compileTime: true,
			description: "Preserve original brightness",
			transform: (v) => v ? 1 : 0,
			ui: {
				type: "checkbox",
				label: "Preserve Luminosity",
				group: "Effect"
			}
		}
	},
	effect: tint({
		color: p("color"),
		amount: p("amount"),
		preserveLuminosity: p("preserveLuminosity")
	}),
	missingChildMessage: "You must pass a child component into the Tint shader."
});
var Tint_default = componentDefinition;
export { componentDefinition as n, Tint_default as t };
