import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "BarnDoors",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Split the content along a center line and wipe outward in both directions",
	props: {
		progress: {
			default: .5,
			description: "How far the doors have opened (0 = fully visible, 1 = fully wiped away)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Progress",
				group: "Transition"
			}
		},
		angle: {
			default: 0,
			description: "Direction the doors open in degrees (0 = apart horizontally)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Transition"
			}
		},
		feather: {
			default: .1,
			description: "Softness of the wipe edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Feather",
				group: "Transition"
			}
		},
		invert: {
			default: false,
			transform: transformBoolean,
			description: "Close in from both edges instead of opening from the center",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.directional(p("angle")).folded(),
		progress: p("progress"),
		feather: p("feather"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Barn Doors shader."
});
var BarnDoors_default = componentDefinition;
export { componentDefinition as n, BarnDoors_default as t };
