import { Ac as defineStd, as as cross } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Cross",
	description: "Plus / cross shape with adjustable arm length, width, and rounding",
	shape: {
		distance: ({ x, y }, u) => cross(x, y, u("radius"), u("thickness"), u("rounding")),
		colorDescription: "Fill color of the cross",
		centerDescription: "Center position of the cross",
		rotationDescription: "Rotation in degrees (45° turns a plus into an ×)",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .35,
				description: "Arm half-length — distance from center to the end of each arm",
				ui: {
					type: "range",
					min: 0,
					max: 1,
					step: .01,
					label: "Arm Length",
					group: "Shape"
				}
			},
			thickness: {
				default: .08,
				description: "Arm half-width — controls how wide each arm is",
				ui: {
					type: "range",
					min: .01,
					max: .5,
					step: .005,
					label: "Arm Width",
					group: "Shape"
				}
			},
			rounding: {
				default: 0,
				description: "Corner rounding — rounds the arm ends and concave corners",
				ui: {
					type: "range",
					min: 0,
					max: .2,
					step: .005,
					label: "Rounding",
					group: "Shape"
				}
			}
		}
	}
});
var Cross_default = componentDefinition;
export { componentDefinition as n, Cross_default as t };
