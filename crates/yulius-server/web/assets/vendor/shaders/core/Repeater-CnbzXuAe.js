import { Ac as defineStd, Ml as p, dt as grid, ft as line, mt as repeatInstances, pt as radial, ut as byMode, ys as crosses } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Repeater",
	role: "structural",
	species: "custom",
	category: "Distortions",
	description: "Repeat the child content in grid, radial or linear layouts with per-instance variation",
	requiresRTT: true,
	requiresChild: true,
	wantsBoundsParams: true,
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		mode: {
			default: "grid",
			transform: (value) => ({
				grid: 0,
				radial: 1,
				linear: 2
			})[value] ?? 0,
			compileTime: true,
			description: "Layout used to place the repeated instances",
			ui: {
				type: "select",
				options: [
					{
						label: "Grid",
						value: "grid"
					},
					{
						label: "Radial",
						value: "radial"
					},
					{
						label: "Linear",
						value: "linear"
					}
				],
				label: "Mode",
				group: "Layout"
			}
		},
		cropLeft: {
			default: 0,
			description: "Crop inset from the left edge of the source element (fraction of its width)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .49,
				step: .005,
				label: "Crop Left",
				group: "Source"
			}
		},
		cropRight: {
			default: 0,
			description: "Crop inset from the right edge of the source element (fraction of its width)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .49,
				step: .005,
				label: "Crop Right",
				group: "Source"
			}
		},
		cropTop: {
			default: 0,
			description: "Crop inset from the top edge of the source element (fraction of its height)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .49,
				step: .005,
				label: "Crop Top",
				group: "Source"
			}
		},
		cropBottom: {
			default: 0,
			description: "Crop inset from the bottom edge of the source element (fraction of its height)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .49,
				step: .005,
				label: "Crop Bottom",
				group: "Source"
			}
		},
		columns: {
			default: 3,
			description: "Number of grid columns",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 12,
				step: 1,
				label: "Columns",
				group: "Layout",
				condition: { mode: "grid" }
			}
		},
		rows: {
			default: 3,
			description: "Number of grid rows",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 12,
				step: 1,
				label: "Rows",
				group: "Layout",
				condition: { mode: "grid" }
			}
		},
		gapX: {
			default: .05,
			description: "Horizontal gap between grid cells",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .5,
				step: .005,
				label: "Gap X",
				group: "Layout",
				condition: { mode: "grid" },
				dimensional: "canvas-width"
			}
		},
		gapY: {
			default: .05,
			description: "Vertical gap between grid cells",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .5,
				step: .005,
				label: "Gap Y",
				group: "Layout",
				condition: { mode: "grid" },
				dimensional: "canvas-height"
			}
		},
		stagger: {
			default: 0,
			description: "Horizontal offset applied to every other row (brick layout)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Stagger",
				group: "Layout",
				condition: { mode: "grid" }
			}
		},
		flip: {
			default: "none",
			transform: (value) => ({
				"none": 0,
				"alternate-flip-x": 1,
				"alternate-flip-y": 2,
				"both": 3
			})[value] ?? 0,
			compileTime: true,
			description: "Mirror alternating columns/rows for seamless tiling patterns",
			ui: {
				type: "select",
				options: [
					{
						label: "None",
						value: "none"
					},
					{
						label: "Alternate Flip X",
						value: "alternate-flip-x"
					},
					{
						label: "Alternate Flip Y",
						value: "alternate-flip-y"
					},
					{
						label: "Both",
						value: "both"
					}
				],
				label: "Flip",
				group: "Layout",
				condition: { mode: "grid" }
			}
		},
		count: {
			default: 8,
			description: "Number of repeated instances",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 64,
				step: 1,
				label: "Count",
				group: "Layout",
				condition: { mode: ["radial", "linear"] }
			}
		},
		radius: {
			default: .3,
			description: "Orbit radius of the radial layout",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Radius",
				group: "Layout",
				condition: { mode: "radial" },
				dimensional: "canvas-height"
			}
		},
		startAngle: {
			default: 0,
			description: "Angle of the first instance on the orbit",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Start Angle",
				group: "Layout",
				condition: { mode: "radial" }
			}
		},
		sweep: {
			default: 360,
			description: "Angular span the instances are distributed across",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Sweep",
				group: "Layout",
				condition: { mode: "radial" }
			}
		},
		faceCenter: {
			default: false,
			transform: transformBoolean,
			description: "Rotate each instance to face the orbit center",
			ui: {
				type: "checkbox",
				label: "Face Center",
				group: "Layout",
				condition: { mode: "radial" }
			}
		},
		direction: {
			default: 0,
			description: "Direction of the linear layout in degrees (0 = rightward)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Direction",
				group: "Layout",
				condition: { mode: "linear" }
			}
		},
		spacing: {
			default: .2,
			description: "Distance between consecutive instances",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Spacing",
				group: "Layout",
				condition: { mode: "linear" },
				dimensional: "canvas-height"
			}
		},
		instanceScale: {
			default: 1,
			description: "Progressive scale multiplier applied per instance (compounds with each copy)",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 2,
				step: .005,
				label: "Scale",
				group: "Instances"
			}
		},
		instanceRotation: {
			default: 0,
			transform: (value) => value * Math.PI / 180,
			description: "Additional rotation in degrees applied per instance (accumulates with each copy)",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Rotation",
				group: "Instances"
			}
		},
		instanceOpacity: {
			default: 1,
			description: "Progressive opacity falloff applied per instance (compounds with each copy)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Opacity",
				group: "Instances"
			}
		},
		hueShift: {
			default: 0,
			transform: (value) => value * Math.PI / 180,
			recompile: crosses(0),
			description: "Hue rotation in degrees added per instance",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Hue Shift",
				group: "Instances"
			}
		},
		phase: {
			default: 0,
			description: "Seamless layout animation phase (0-1 wraps to the identical layout)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .001,
				label: "Phase",
				group: "Instances"
			}
		},
		zOrder: {
			default: "forward",
			transform: (value) => value === "backward" ? 1 : 0,
			compileTime: true,
			description: "Stacking order of overlapping instances",
			ui: {
				type: "select",
				options: [{
					label: "Forward",
					value: "forward"
				}, {
					label: "Backward",
					value: "backward"
				}],
				label: "Z-Order",
				group: "Instances"
			}
		},
		jitterPosition: {
			default: 0,
			description: "Random per-instance position offset",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Position",
				group: "Jitter"
			}
		},
		jitterRotation: {
			default: 0,
			description: "Random per-instance rotation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Rotation",
				group: "Jitter"
			}
		},
		jitterScale: {
			default: 0,
			description: "Random per-instance scale variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Scale",
				group: "Jitter"
			}
		},
		jitterOpacity: {
			default: 0,
			description: "Random per-instance opacity variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Opacity",
				group: "Jitter"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for the jitter values",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Jitter"
			}
		}
	},
	gpu: { fragment: repeatInstances({
		source: { crop: {
			left: p("cropLeft"),
			right: p("cropRight"),
			top: p("cropTop"),
			bottom: p("cropBottom")
		} },
		placement: byMode(p("mode"), {
			grid: grid({
				columns: p("columns"),
				rows: p("rows"),
				gapX: p("gapX"),
				gapY: p("gapY"),
				stagger: p("stagger"),
				flip: p("flip")
			}),
			radial: radial({
				count: p("count"),
				radius: p("radius"),
				startAngle: p("startAngle"),
				sweep: p("sweep"),
				faceCenter: p("faceCenter")
			}),
			linear: line({
				count: p("count"),
				direction: p("direction"),
				spacing: p("spacing")
			})
		}),
		variation: {
			scale: p("instanceScale"),
			rotation: p("instanceRotation"),
			opacity: p("instanceOpacity"),
			jitter: {
				position: p("jitterPosition"),
				rotation: p("jitterRotation"),
				scale: p("jitterScale"),
				opacity: p("jitterOpacity")
			},
			seed: p("seed"),
			phase: p("phase")
		},
		hueShift: p("hueShift"),
		order: p("zOrder")
	}) }
});
var Repeater_default = componentDefinition;
export { componentDefinition as n, Repeater_default as t };
