import { Ac as defineStd, dc as mul, rs as circle } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Circle",
	description: "Generate a circle with adjustable size and softness",
	shape: {
		distance: ({ x, y }, u) => circle(x, y, mul(u("radius"), .5)),
		rotatable: false,
		colorDescription: "The color of the circle",
		centerDescription: "The center point of the circle",
		centerLabel: "Center Position",
		bounds: { size: {
			width: "radius",
			height: "radius",
			as: "canvas-height"
		} },
		shapeProps: { radius: {
			default: 1,
			description: "The radius of the circle. A value of one (1) is sets the circle to fit the canvas.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Radius",
				group: "Effect",
				units: ["%", "px"]
			}
		} },
		stroke: {
			strokeColorDescription: "The color of the stroke outline",
			strokePositionDescription: "Position of the stroke relative to the circle edge"
		},
		colorSpaceDescription: "Color space for blending fill and stroke colors in soft edges",
		propOverrides: {
			softness: {
				default: 0,
				description: "Edge softness. Lower values like zero (0) are sharp, higher values like one (1) are softer.",
				ui: {
					type: ["range", "map"],
					min: 0,
					max: 1,
					step: .01,
					label: "Softness",
					group: "Effect",
					dimensional: "canvas-height"
				}
			},
			strokeThickness: {
				default: 0,
				description: "The thickness of the stroke outline. Zero (0) means no stroke.",
				ui: {
					type: ["range", "map"],
					min: 0,
					max: .5,
					step: .01,
					label: "Stroke Thickness",
					group: "Stroke"
				}
			}
		},
		propOrder: [
			"origin",
			"color",
			"radius",
			"softness",
			"center",
			"strokeThickness",
			"strokeColor",
			"strokePosition",
			"colorSpace"
		]
	}
});
var Circle_default = componentDefinition;
export { componentDefinition as n, Circle_default as t };
