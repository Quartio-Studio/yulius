import { Ac as defineStd, Ml as p, Qn as slicedSlide } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "SliceWipe",
	role: "warp",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Transitions",
	description: "Slice the content into strips that slide away in alternating directions",
	props: {
		progress: {
			default: .5,
			description: "How far the strips have slid (0 = fully visible, 1 = fully wiped away)",
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
			description: "Orientation of the strips in degrees (0 = vertical strips sliding up and down)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Transition"
			}
		},
		sliceCount: {
			default: 8,
			description: "Number of strips across the frame",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 60,
				step: 1,
				label: "Slices",
				group: "Transition"
			}
		}
	},
	map: slicedSlide({
		angle: p("angle"),
		count: p("sliceCount"),
		progress: p("progress")
	}),
	edges: 1,
	resample: "bilinear",
	missingChildMessage: "You must pass a child component into the Slice Wipe shader."
});
var SliceWipe_default = componentDefinition;
export { componentDefinition as n, SliceWipe_default as t };
