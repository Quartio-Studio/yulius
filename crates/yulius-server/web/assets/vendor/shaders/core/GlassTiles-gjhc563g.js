import { Ac as defineStd, Ad as glassTilesRemapUV, Al as uniformOf, Bd as ZERO, Ml as p, kd as glassTilesFragment } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "GlassTiles",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Distortions",
	description: "Refraction-like distortion in a tile grid pattern",
	requiresRTT: true,
	requiresChild: true,
	props: {
		intensity: {
			default: 2,
			description: "The intensity of the glass tiles effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		tileCount: {
			default: 20,
			description: "Number of tiles across the longest dimension",
			ui: {
				type: ["range", "map"],
				min: 5,
				max: 50,
				step: 1,
				label: "Tile Count",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation angle of the tile grid in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		roundness: {
			default: 0,
			description: "Makes tiles more circular instead of square",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Roundness",
				group: "Effect"
			}
		}
	},
	gpu: {
		fragment: (params) => {
			if (!params.childNode) {
				console.error("You must pass a child component into the Glass Tiles shader.");
				return ZERO;
			}
			return glassTilesFragment({
				uv: params.ctx.uv,
				aspect: params.ctx.aspect,
				intensity: uniformOf(p("intensity"), params),
				tileCount: uniformOf(p("tileCount"), params),
				rotation: uniformOf(p("rotation"), params),
				roundness: uniformOf(p("roundness"), params),
				texture: params.convertToTexture(params.childNode)
			});
		},
		uvRemap: ({ uv, mask, uniforms, aspect }) => ({
			uv: glassTilesRemapUV({
				uv,
				aspect,
				intensity: uniformOf(p("intensity"), { uniforms }),
				tileCount: uniformOf(p("tileCount"), { uniforms }),
				rotation: uniformOf(p("rotation"), { uniforms }),
				roundness: uniformOf(p("roundness"), { uniforms })
			}),
			mask
		})
	}
});
var GlassTiles_default = componentDefinition;
export { componentDefinition as n, GlassTiles_default as t };
