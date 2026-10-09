import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "VenetianBlinds",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away behind a set of parallel closing strips",
	props: {
		progress: {
			default: .5,
			description: "How far the blinds have closed (0 = fully visible, 1 = fully wiped away)",
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
			description: "Orientation of the strips in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Transition"
			}
		},
		stripCount: {
			default: 5,
			description: "Number of strips across the frame",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 60,
				step: 1,
				label: "Strips",
				group: "Transition"
			}
		},
		feather: {
			default: .15,
			description: "Softness of each closing strip edge",
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
			description: "Close each strip from the opposite edge",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.directional(p("angle")).tiled(p("stripCount")),
		progress: p("progress"),
		feather: p("feather"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Venetian Blinds shader."
});
var VenetianBlinds_default = componentDefinition;
export { componentDefinition as n, VenetianBlinds_default as t };
