import { Ac as defineStd, Ml as p, X as reveal, Y as coverage } from "./std-ClftfWpF.js";
import { c as transformPosition, i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "RippleWipe",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Wipe the content away in concentric rings pulsing out from a center point",
	props: {
		progress: {
			default: .5,
			description: "How far the ripple has expanded (0 = fully visible, 1 = fully wiped away)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Progress",
				group: "Transition"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Point the rings expand from",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		rings: {
			default: 8,
			description: "Number of concentric rings",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 40,
				step: 1,
				label: "Rings",
				group: "Transition"
			}
		},
		feather: {
			default: .2,
			description: "How softly adjacent rings' timing overlaps (0 = discrete ring pops)",
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
			description: "Wipe from the outside inward instead",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Transition"
			}
		}
	},
	effect: reveal({
		coverage: coverage.radial(p("center")).bands(p("rings")),
		progress: p("progress"),
		feather: p("feather"),
		invert: p("invert")
	}),
	missingChildMessage: "You must pass a child component into the Ripple Wipe shader."
});
var RippleWipe_default = componentDefinition;
export { componentDefinition as n, RippleWipe_default as t };
