import { Ac as defineStd, rs as circle, uc as mix, us as polygon } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Polygon",
	description: "Regular polygon with adjustable sides and corner rounding",
	shape: {
		distance: ({ x, y }, u) => mix(polygon(x, y, u("radius"), u("sides")), circle(x, y, u("radius")), u("rounding")),
		colorDescription: "Fill color of the polygon",
		centerDescription: "Center position of the polygon",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .4,
				description: "Inradius — distance from the center to the middle of each side, in UV space (the vertices sit a little farther out)",
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
				default: 6,
				description: "Number of sides (3 = triangle, 4 = square, 6 = hexagon, etc.)",
				ui: {
					type: "range",
					min: 3,
					max: 12,
					step: 1,
					label: "Sides",
					group: "Shape"
				}
			},
			rounding: {
				default: 0,
				description: "Corner rounding — 0 is sharp vertices, 1 morphs into a circle",
				ui: {
					type: "range",
					min: 0,
					max: 1,
					step: .01,
					label: "Rounding",
					group: "Shape"
				}
			}
		}
	}
});
var Polygon_default = componentDefinition;
export { componentDefinition as n, Polygon_default as t };
