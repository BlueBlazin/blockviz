package api

import (
	"net/http"

	"github.com/gin-gonic/gin"
	"github.com/jmoiron/sqlx"
)

// Product is a row of the products table.
type Product struct {
	ID    int64   `db:"id" json:"id"`
	Name  string  `db:"name" json:"name"`
	Price float64 `db:"price" json:"price"`
}

// Server serves the shop's API.
type Server struct {
	db     *sqlx.DB
	router *gin.Engine
}

// New wires the routes.
func New(db *sqlx.DB) *Server {
	s := &Server{db: db, router: gin.Default()}
	api := s.router.Group("/api")
	v1 := api.Group("/v1")
	v1.GET("/products", s.listProducts)
	v1.GET("/products/:id", s.getProduct)
	v1.POST("/products", s.createProduct)
	return s
}

func (s *Server) listProducts(c *gin.Context) {
	var products []Product
	if err := s.db.Select(&products, "SELECT * FROM products"); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusOK, products)
}

func (s *Server) getProduct(c *gin.Context) {
	var p Product
	if err := s.db.Get(&p, "SELECT * FROM products WHERE id = $1", c.Param("id")); err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, p)
}

func (s *Server) createProduct(c *gin.Context) {
	var p Product
	if err := c.ShouldBindJSON(&p); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	if _, err := s.db.NamedExec("INSERT INTO products (name, price) VALUES (:name, :price)", &p); err != nil {
		c.AbortWithStatus(http.StatusInternalServerError)
		return
	}
	c.JSON(http.StatusCreated, p)
}

// Run serves on addr.
func (s *Server) Run(addr string) error {
	return s.router.Run(addr)
}
