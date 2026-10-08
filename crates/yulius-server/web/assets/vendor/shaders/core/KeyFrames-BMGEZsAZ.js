import { Ac as defineStd, Ml as p, ld as createTrackerPursuitSim, st as motionTrackerHud } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "KeyFrames",
	role: "overlay",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Motion-tracking overlay — trackers hunt and follow features in the content, dropping keyframe diamonds behind them along their motion paths, like a compositor mid-track",
	requiresRTT: true,
	requiresChild: true,
	props: {
		trackers: {
			default: 12,
			description: "Number of simultaneous trackers.",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 64,
				step: 1,
				label: "Trackers",
				group: "Tracking"
			}
		},
		detect: {
			default: "bright",
			compileTime: true,
			description: "What the trackers hunt for.",
			ui: {
				type: "select",
				options: [
					{
						label: "Brightness",
						value: "bright"
					},
					{
						label: "Darkness",
						value: "dark"
					},
					{
						label: "Transparency",
						value: "alpha"
					},
					{
						label: "Red",
						value: "red"
					},
					{
						label: "Green",
						value: "green"
					},
					{
						label: "Blue",
						value: "blue"
					}
				],
				label: "Detect",
				group: "Tracking"
			}
		},
		threshold: {
			default: .15,
			description: "Feature floor — content scoring below this doesn't count as trackable at all.",
			ui: {
				type: "range",
				min: 0,
				max: .9,
				step: .01,
				label: "Threshold",
				group: "Tracking"
			}
		},
		variance: {
			default: .8,
			description: "Spread of tracker preferences — 0 hunts only the strongest features, 1 distributes trackers across the whole brightness range.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Variance",
				group: "Tracking"
			}
		},
		agility: {
			default: .5,
			description: "How fast trackers travel and how wide they search.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Agility",
				group: "Tracking"
			}
		},
		lifespan: {
			default: 6,
			description: "Seconds a tracker follows its target before retiring and reacquiring elsewhere. 0 = track forever.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .5,
				label: "Lifespan",
				group: "Tracking"
			}
		},
		trail: {
			default: 1,
			description: "Visibility of the keyframe trail each tracker leaves behind.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Trail",
				group: "Style"
			}
		},
		markerSize: {
			default: 28,
			description: "Tracker gizmo size in pixels.",
			ui: {
				type: "range",
				min: 8,
				max: 80,
				step: 1,
				label: "Marker Size",
				group: "Style"
			}
		},
		lineWidth: {
			default: 1.5,
			description: "Stroke width in pixels.",
			ui: {
				type: "range",
				min: .5,
				max: 6,
				step: .25,
				label: "Line Width",
				group: "Style"
			}
		},
		markerColor: {
			default: "#54d66b",
			transform: transformColor,
			description: "Tracker gizmo color.",
			ui: {
				type: "color",
				label: "Tracker",
				group: "Colors"
			}
		},
		keyframeColor: {
			default: "#ffcf33",
			transform: transformColor,
			description: "Keyframe diamond color.",
			ui: {
				type: "color",
				label: "Keyframes",
				group: "Colors"
			}
		},
		pathColor: {
			default: "#ffffffb8",
			transform: transformColor,
			description: "Motion path color.",
			ui: {
				type: "color",
				label: "Path",
				group: "Colors"
			}
		}
	},
	compute: (params) => createTrackerPursuitSim({
		mode: typeof params.propValues.detect === "string" ? params.propValues.detect : "bright",
		trackers: "trackers",
		agility: "agility",
		threshold: "threshold",
		variance: "variance",
		lifespan: "lifespan"
	})(params),
	gpu: { fragment: motionTrackerHud({
		trackers: p("trackers"),
		trail: p("trail"),
		markerSize: p("markerSize"),
		lineWidth: p("lineWidth"),
		markerColor: p("markerColor"),
		keyframeColor: p("keyframeColor"),
		pathColor: p("pathColor")
	}) }
});
var KeyFrames_default = componentDefinition;
export { componentDefinition as n, KeyFrames_default as t };
