import { Ac as defineStd, Ml as p, Pl as edgesPropConfig, Rn as barOffset } from "./std-ClftfWpF.js";
import { r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "BarShift",
	role: "warp",
	category: "Distortions",
	description: "Slices content into parallel bars, each offset independently for a fractured or glitch-like effect",
	boundingBoxDeclaration: { aspectRatio: null },
	animatedTime: { speed: "speed" },
	props: {
		count: {
			default: 6,
			description: "Number of bars across the longest viewport dimension",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Count",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Angle of bar orientation in degrees (0 = vertical bars, 90 = horizontal bars)",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		intensity: {
			default: .15,
			description: "Maximum displacement per bar",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Randomization seed for per-bar offset variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Animation speed — each bar drifts at its own rate and direction",
			ui: {
				type: "range",
				min: -2,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		edges: edgesPropConfig("mirror", "How to handle edges when distortion pushes content out of bounds")
	},
	map: barOffset({
		count: p("count"),
		angle: p("angle"),
		intensity: p("intensity"),
		seed: p("seed")
	}),
	missingChildMessage: "You must pass a child component into the BarShift shader."
});
var BarShift_default = componentDefinition;
export { componentDefinition as n, BarShift_default as t };
