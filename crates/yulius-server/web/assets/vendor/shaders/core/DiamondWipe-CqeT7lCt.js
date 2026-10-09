import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "DiamondWipe",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away through a lattice of growing diamonds",
	props: {
		progress: {
			default: .5,
			description: "How far the diamonds have grown (0 = fully visible, 1 = fully wiped away)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Progress",
				group: "Transition"
			}
		},
		size: {
			default: .15,
			description: "Size of each diamond cell as a fraction of the frame width",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: .5,
				step: .01,
				label: "Size",
				group: "Transition"
			}
		},
		feather: {
			default: .1,
			description: "Softness of each diamond edge",
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
			description: "Wipe from the cell corners inward instead",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.cells(p("size")).diamond(),
		progress: p("progress"),
		feather: p("feather"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Diamond Wipe shader."
});
var DiamondWipe_default = componentDefinition;
export { componentDefinition as n, DiamondWipe_default as t };
