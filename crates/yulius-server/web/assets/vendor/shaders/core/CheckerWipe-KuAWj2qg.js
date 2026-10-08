import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "CheckerWipe",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away as a checkerboard of fading squares",
	props: {
		progress: {
			default: .5,
			description: "How far the wipe has progressed (0 = fully visible, 1 = fully wiped away)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Progress",
				group: "Transition"
			}
		},
		blockSize: {
			default: .1,
			description: "Size of each square as a fraction of the frame width",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: .5,
				step: .01,
				label: "Block Size",
				group: "Transition"
			}
		},
		softness: {
			default: .15,
			description: "How softly each square fades out (0 = hard-edged squares)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Transition"
			}
		},
		invert: {
			default: false,
			transform: transformBoolean,
			description: "Reverse the order the squares wipe in",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.cells(p("blockSize")).checker(),
		progress: p("progress"),
		feather: p("softness"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Checker Wipe shader."
});
var CheckerWipe_default = componentDefinition;
export { componentDefinition as n, CheckerWipe_default as t };
