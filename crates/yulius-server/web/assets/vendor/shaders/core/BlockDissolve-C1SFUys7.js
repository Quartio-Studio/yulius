import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "BlockDissolve",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Dissolve the content away as a grid of blocks vanishing in random order",
	props: {
		progress: {
			default: .5,
			description: "How far the dissolve has progressed (0 = fully visible, 1 = fully wiped away)",
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
			default: .08,
			description: "Size of each block as a fraction of the frame width",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: .5,
				step: .01,
				label: "Block Size",
				group: "Transition"
			}
		},
		softness: {
			default: .15,
			description: "How softly each block fades out (0 = hard-edged blocks)",
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
			description: "Reverse the order blocks dissolve in",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.cells(p("blockSize")).shuffled(),
		progress: p("progress"),
		feather: p("softness"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Block Dissolve shader."
});
var BlockDissolve_default = componentDefinition;
export { componentDefinition as n, BlockDissolve_default as t };
