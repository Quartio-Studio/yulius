import { Ac as defineStd, ms as star } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Star",
	description: "Classic star polygon with straight sides and sharp pointed tips",
	shape: {
		distance: ({ x, y }, u) => star(x, y, u("radius"), u("sides"), u("innerRatio")),
		colorDescription: "Fill color of the star",
		centerDescription: "Center position of the star",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .4,
				description: "Outer tip radius — distance from center to the pointed tips",
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
				description: "Number of points on the star",
				ui: {
					type: "range",
					min: 3,
					max: 12,
					step: 1,
					label: "Points",
					group: "Shape"
				}
			},
			innerRatio: {
				default: .4,
				description: "Inner vertex radius as a ratio of outer radius (0.382 = golden-ratio 5-star)",
				ui: {
					type: "range",
					min: .1,
					max: .9,
					step: .01,
					label: "Inner Ratio",
					group: "Shape"
				}
			}
		}
	}
});
var Star_default = componentDefinition;
export { componentDefinition as n, Star_default as t };
