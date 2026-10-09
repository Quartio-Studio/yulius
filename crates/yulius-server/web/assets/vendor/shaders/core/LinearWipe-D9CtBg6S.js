import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "LinearWipe",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away along a straight edge with a soft feathered transition",
	props: {
		progress: {
			default: .5,
			description: "How far the wipe has travelled (0 = fully visible, 1 = fully wiped away)",
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
			description: "Direction of the wipe in degrees (0 = left to right)",
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
			description: "Softness of the wipe edge",
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
			description: "Reverse the direction the wipe travels",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.directional(p("angle")),
		progress: p("progress"),
		feather: p("feather"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Linear Wipe shader."
});
var LinearWipe_default = componentDefinition;
export { componentDefinition as n, LinearWipe_default as t };
