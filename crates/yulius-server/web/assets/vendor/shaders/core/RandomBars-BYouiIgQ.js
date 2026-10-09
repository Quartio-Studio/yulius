import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "RandomBars",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away as parallel bars vanishing in random order",
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
		angle: {
			default: 0,
			description: "Orientation of the bars in degrees (0 = vertical bars)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Transition"
			}
		},
		barCount: {
			default: 12,
			description: "Number of bars across the frame",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 100,
				step: 1,
				label: "Bars",
				group: "Transition"
			}
		},
		softness: {
			default: .15,
			description: "How softly each bar fades out (0 = hard-edged bars)",
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
			description: "Reverse the order the bars vanish in",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.directional(p("angle")).bands(p("barCount")).shuffled(),
		progress: p("progress"),
		feather: p("softness"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Random Bars shader."
});
var RandomBars_default = componentDefinition;
export { componentDefinition as n, RandomBars_default as t };
