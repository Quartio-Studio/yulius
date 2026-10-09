import { Ac as defineStd, ss as flower } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Flower",
	description: "Petal shape with N lobes and adjustable inner-to-outer radius ratio",
	shape: {
		distance: ({ x, y }, u) => flower(x, y, u("radius"), u("sides"), u("innerRatio")),
		colorDescription: "Fill color of the flower",
		centerDescription: "Center position of the flower",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .4,
				description: "Outer petal tip radius in UV space",
				ui: {
					type: "range",
					min: 0,
					max: 1,
					step: .01,
					label: "Radius",
					group: "Shape"
				}
			},
			sides: {
				default: 5,
				description: "Number of petals",
				ui: {
					type: "range",
					min: 3,
					max: 12,
					step: 1,
					label: "Petals",
					group: "Shape"
				}
			},
			innerRatio: {
				default: .4,
				description: "Inner valley radius as a ratio of outer radius — lower values make deeper notches",
				ui: {
					type: "range",
					min: .1,
					max: .95,
					step: .01,
					label: "Inner Ratio",
					group: "Shape"
				}
			}
		}
	}
});
var Flower_default = componentDefinition;
export { componentDefinition as n, Flower_default as t };
