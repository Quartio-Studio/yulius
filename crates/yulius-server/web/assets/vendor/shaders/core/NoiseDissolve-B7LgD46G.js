import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "NoiseDissolve",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Dissolve the content away through an organic noise pattern",
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
		scale: {
			default: 3,
			description: "Frequency of the dissolve pattern (higher = smaller blobs)",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 20,
				step: .1,
				label: "Scale",
				group: "Transition"
			}
		},
		softness: {
			default: .25,
			description: "How softly the erosion edge fades out (0 = hard-edged blobs)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Transition"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for pattern variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Transition"
			}
		},
		invert: {
			default: false,
			transform: transformBoolean,
			description: "Reverse the order the pattern dissolves in",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.noise(p("scale"), p("seed")),
		progress: p("progress"),
		feather: p("softness"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Noise Dissolve shader."
});
var NoiseDissolve_default = componentDefinition;
export { componentDefinition as n, NoiseDissolve_default as t };
