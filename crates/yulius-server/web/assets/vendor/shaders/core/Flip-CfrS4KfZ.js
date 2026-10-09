import { Ac as defineStd, Ml as p, Wn as flip } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Flip",
	role: "warp",
	category: "Distortions",
	description: "Mirror content horizontally, vertically, or both",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		flipX: {
			default: false,
			transform: transformBoolean,
			description: "Mirror the content horizontally (left ↔ right)",
			ui: {
				type: "checkbox",
				label: "Flip X",
				group: "Effect"
			}
		},
		flipY: {
			default: false,
			transform: transformBoolean,
			description: "Mirror the content vertically (top ↔ bottom)",
			ui: {
				type: "checkbox",
				label: "Flip Y",
				group: "Effect"
			}
		}
	},
	map: flip({
		flipX: p("flipX"),
		flipY: p("flipY")
	}),
	edges: "none",
	missingChildMessage: "You must pass a child component into the Flip shader."
});
var Flip_default = componentDefinition;
export { componentDefinition as n, Flip_default as t };
